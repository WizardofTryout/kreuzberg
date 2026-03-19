use std::borrow::Cow;

use crate::{
    ocr_error::OcrError,
    ocr_result::{Point, TextBox},
};
use image::imageops;
use imageproc::geometric_transformations::{Interpolation, Projection};
use ndarray::{Array, Array4};

pub struct OcrUtils;

impl OcrUtils {
    /// Normalize image pixels and transpose from HWC (row-major RGB) to CHW tensor format.
    ///
    /// Formula per pixel: `output[ch] = pixel[ch] * norm[ch] - mean[ch] * norm[ch]`
    ///
    /// Optimizations over the original implementation:
    /// - Pre-computes `mean * norm` constants (was recomputed per-pixel)
    /// - Uses raw slice access (avoids per-pixel bounds checks)
    /// - Row-major iteration with pre-computed row offsets (cache-friendly)
    pub fn substract_mean_normalize(img_src: &image::RgbImage, mean_vals: &[f32], norm_vals: &[f32]) -> Array4<f32> {
        let cols = img_src.width() as usize;
        let rows = img_src.height() as usize;

        let mut input_tensor = Array::zeros((1, 3, rows, cols));

        // Pre-compute adjusted mean constants: mean[ch] * norm[ch]
        // This avoids a multiply per pixel per channel (was 3× per pixel before).
        let adjusted = [
            mean_vals[0] * norm_vals[0],
            mean_vals[1] * norm_vals[1],
            mean_vals[2] * norm_vals[2],
        ];

        // Access raw pixel buffer directly — avoids per-pixel bounds checks.
        // RgbImage stores pixels in row-major HWC format: [R, G, B, R, G, B, ...].
        let raw = img_src.as_raw();

        for r in 0..rows {
            let row_offset = r * cols * 3;
            for c in 0..cols {
                let idx = row_offset + c * 3;
                // Channel-separated output (CHW format for ONNX Runtime).
                input_tensor[[0, 0, r, c]] = raw[idx] as f32 * norm_vals[0] - adjusted[0];
                input_tensor[[0, 1, r, c]] = raw[idx + 1] as f32 * norm_vals[1] - adjusted[1];
                input_tensor[[0, 2, r, c]] = raw[idx + 2] as f32 * norm_vals[2] - adjusted[2];
            }
        }

        input_tensor
    }

    /// Add white padding around the image, or borrow it unchanged when padding=0.
    /// Returns Cow to avoid cloning the image in the common no-padding case.
    pub fn make_padding<'a>(img_src: &'a image::RgbImage, padding: u32) -> Result<Cow<'a, image::RgbImage>, OcrError> {
        if padding == 0 {
            return Ok(Cow::Borrowed(img_src));
        }

        let width = img_src.width();
        let height = img_src.height();

        let mut padding_src = image::RgbImage::new(width + 2 * padding, height + 2 * padding);
        imageproc::drawing::draw_filled_rect_mut(
            &mut padding_src,
            imageproc::rect::Rect::at(0, 0).of_size(width + 2 * padding, height + 2 * padding),
            image::Rgb([255, 255, 255]),
        );

        image::imageops::replace(&mut padding_src, img_src, padding as i64, padding as i64);

        Ok(Cow::Owned(padding_src))
    }

    pub fn get_part_images(img_src: &image::RgbImage, text_boxes: &[TextBox]) -> Vec<image::RgbImage> {
        text_boxes
            .iter()
            .map(|text_box| Self::get_rotate_crop_image(img_src, &text_box.points))
            .collect()
    }

    pub fn get_rotate_crop_image(img_src: &image::RgbImage, box_points: &[Point]) -> image::RgbImage {
        let mut points = box_points.to_vec();

        // Calculate bounding box
        let (min_x, min_y, max_x, max_y) = points.iter().fold(
            (u32::MAX, u32::MAX, 0u32, 0u32),
            |(min_x, min_y, max_x, max_y), point| {
                (
                    min_x.min(point.x),
                    min_y.min(point.y),
                    max_x.max(point.x),
                    max_y.max(point.y),
                )
            },
        );

        // Crop image
        let img_crop = imageops::crop_imm(img_src, min_x, min_y, max_x - min_x, max_y - min_y).to_image();

        for point in &mut points {
            point.x = point.x.saturating_sub(min_x);
            point.y = point.y.saturating_sub(min_y);
        }

        // Ensure we have enough points for transformation
        if points.len() < 4 {
            // Fallback: return the cropped image as-is if we don't have 4 points
            return img_crop;
        }

        // Direct multiplication instead of .pow(2) — avoids integer power function overhead.
        let dx_w = (points[0].x as i32 - points[1].x as i32) as f32;
        let dy_w = (points[0].y as i32 - points[1].y as i32) as f32;
        let img_crop_width = (dx_w * dx_w + dy_w * dy_w).sqrt() as u32;
        let dx_h = (points[0].x as i32 - points[3].x as i32) as f32;
        let dy_h = (points[0].y as i32 - points[3].y as i32) as f32;
        let img_crop_height = (dx_h * dx_h + dy_h * dy_h).sqrt() as u32;

        // Ensure dimensions are valid (non-zero)
        if img_crop_width == 0 || img_crop_height == 0 {
            return img_crop;
        }

        let src_points = [
            (points[0].x as f32, points[0].y as f32),
            (points[1].x as f32, points[1].y as f32),
            (points[2].x as f32, points[2].y as f32),
            (points[3].x as f32, points[3].y as f32),
        ];

        let dst_points = [
            (0.0, 0.0),
            (img_crop_width as f32, 0.0),
            (img_crop_width as f32, img_crop_height as f32),
            (0.0, img_crop_height as f32),
        ];

        let projection = match Projection::from_control_points(src_points, dst_points) {
            Some(proj) => proj,
            None => {
                // If projection cannot be created, return the cropped image as fallback
                return img_crop;
            }
        };

        let mut part_img = image::RgbImage::new(img_crop_width, img_crop_height);
        imageproc::geometric_transformations::warp_into(
            &img_crop,
            &projection,
            Interpolation::Nearest,
            image::Rgb([255, 255, 255]),
            &mut part_img,
        );

        // Rotate image if needed
        if part_img.height() >= part_img.width() * 3 / 2 {
            let mut rotated = image::RgbImage::new(part_img.height(), part_img.width());

            for (x, y, pixel) in part_img.enumerate_pixels() {
                rotated.put_pixel(y, part_img.width() - 1 - x, *pixel);
            }

            rotated
        } else {
            part_img
        }
    }

    pub fn mat_rotate_clock_wise_180(src: &mut image::RgbImage) {
        imageops::rotate180_in_place(src);
    }

    pub fn calculate_mean_with_mask(
        img: &image::ImageBuffer<image::Luma<f32>, Vec<f32>>,
        mask: &image::ImageBuffer<image::Luma<u8>, Vec<u8>>,
    ) -> f32 {
        let mut sum: f32 = 0.0;
        let mut mask_count = 0;

        assert_eq!(img.width(), mask.width());
        assert_eq!(img.height(), mask.height());

        for y in 0..img.height() {
            for x in 0..img.width() {
                let mask_value = mask.get_pixel(x, y)[0];
                if mask_value > 0 {
                    let pixel = img.get_pixel(x, y);
                    sum += pixel[0];
                    mask_count += 1;
                }
            }
        }

        if mask_count == 0 {
            return 0.0;
        }

        sum / mask_count as f32
    }
}
