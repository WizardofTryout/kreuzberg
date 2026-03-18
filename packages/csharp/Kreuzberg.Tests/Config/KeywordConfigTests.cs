using System;
using System.Collections.Generic;
using System.Text.Json;
using Xunit;

namespace Kreuzberg.Tests.Config;

/// <summary>
/// Comprehensive configuration tests for KeywordConfig.
/// Tests keyword extraction algorithms, scoring, and language settings.
/// </summary>
public class KeywordConfigTests
{
    [Fact]
    public void Constructor_ShouldCreateWithDefaults()
    {
        var config = new KeywordConfig();

        Assert.Null(config.Algorithm);
        Assert.Null(config.MaxKeywords);
        Assert.Null(config.MinScore);
        Assert.Null(config.NgramRange);
        Assert.Null(config.Language);
        Assert.Null(config.YakeParams);
        Assert.Null(config.RakeParams);
    }

    [Fact]
    public void Constructor_ShouldCreateWithCustomValues()
    {
        var config = new KeywordConfig
        {
            Algorithm = "yake",
            MaxKeywords = 10,
            MinScore = 0.5,
            Language = "en",
            NgramRange = new List<int> { 1, 3 }
        };

        Assert.Equal("yake", config.Algorithm);
        Assert.Equal(10, config.MaxKeywords);
        Assert.Equal(0.5, config.MinScore);
        Assert.Equal("en", config.Language);
        Assert.Equal(2, config.NgramRange.Count);
    }

    [Fact]
    public void Serialize_ShouldRoundTrip()
    {
        var original = new KeywordConfig
        {
            Algorithm = "rake",
            MaxKeywords = 15,
            MinScore = 0.3,
            Language = "fr"
        };

        var json = JsonSerializer.Serialize(original);
        var restored = JsonSerializer.Deserialize<KeywordConfig>(json);

        Assert.NotNull(restored);
        Assert.Equal(original.Algorithm, restored.Algorithm);
        Assert.Equal(original.MaxKeywords, restored.MaxKeywords);
        Assert.Equal(original.MinScore, restored.MinScore);
        Assert.Equal(original.Language, restored.Language);
    }

    [Theory]
    [InlineData("yake")]
    [InlineData("rake")]
    public void Algorithm_ShouldAcceptValidValues(string algorithm)
    {
        var config = new KeywordConfig { Algorithm = algorithm };

        Assert.Equal(algorithm, config.Algorithm);
    }

    [Fact]
    public void MaxKeywords_ShouldAcceptValidValues()
    {
        var config1 = new KeywordConfig { MaxKeywords = 5 };
        var config2 = new KeywordConfig { MaxKeywords = 20 };
        var config3 = new KeywordConfig { MaxKeywords = 100 };

        Assert.Equal(5, config1.MaxKeywords);
        Assert.Equal(20, config2.MaxKeywords);
        Assert.Equal(100, config3.MaxKeywords);
    }

    [Fact]
    public void MinScore_ShouldAcceptValidRange()
    {
        var config1 = new KeywordConfig { MinScore = 0.0 };
        var config2 = new KeywordConfig { MinScore = 0.5 };
        var config3 = new KeywordConfig { MinScore = 1.0 };

        Assert.Equal(0.0, config1.MinScore);
        Assert.Equal(0.5, config2.MinScore);
        Assert.Equal(1.0, config3.MinScore);
    }

    [Fact]
    public void NgramRange_ShouldSpecifyMinMaxNgrams()
    {
        var config = new KeywordConfig
        {
            NgramRange = new List<int> { 1, 3 }
        };

        Assert.NotNull(config.NgramRange);
        Assert.Equal(2, config.NgramRange.Count);
        Assert.Equal(1, config.NgramRange[0]);
        Assert.Equal(3, config.NgramRange[1]);
    }

    [Theory]
    [InlineData("en")]
    [InlineData("fr")]
    [InlineData("de")]
    [InlineData("es")]
    [InlineData("pt")]
    public void Language_ShouldAcceptValidLanguageCodes(string language)
    {
        var config = new KeywordConfig { Language = language };

        Assert.Equal(language, config.Language);
    }

    [Fact]
    public void YakeParams_ShouldAcceptCustomParameters()
    {
        var config = new KeywordConfig
        {
            YakeParams = new YakeParamsConfig { WindowSize = 10 }
        };

        Assert.NotNull(config.YakeParams);
        Assert.Equal(10, config.YakeParams.WindowSize);
    }

    [Fact]
    public void RakeParams_ShouldAcceptCustomParameters()
    {
        var config = new KeywordConfig
        {
            RakeParams = new RakeParamsConfig
            {
                MinWordLength = 3,
                MaxWordsPerPhrase = 5
            }
        };

        Assert.NotNull(config.RakeParams);
        Assert.Equal(3, config.RakeParams.MinWordLength);
        Assert.Equal(5, config.RakeParams.MaxWordsPerPhrase);
    }

    [Fact]
    public void Nesting_ShouldWorkInExtractionConfig()
    {
        var keywordConfig = new KeywordConfig
        {
            Algorithm = "yake",
            MaxKeywords = 10,
            Language = "en"
        };
        var extractionConfig = new ExtractionConfig { Keywords = keywordConfig };

        Assert.Equal("yake", extractionConfig.Keywords?.Algorithm);
        Assert.Equal(10, extractionConfig.Keywords?.MaxKeywords);
        Assert.Equal("en", extractionConfig.Keywords?.Language);
    }

    [Fact]
    public void AllPropertiesAreInitOnly()
    {
        var properties = typeof(KeywordConfig)
            .GetProperties(System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Instance)
            .Where(p => p.SetMethod != null)
            .ToList();

        Assert.True(properties.Count > 0, "KeywordConfig should have at least one settable property");

        foreach (var prop in properties)
        {
            var hasInitOnly = prop.SetMethod.ReturnParameter?
                .GetRequiredCustomModifiers()
                .Any(m => m.Name == "IsExternalInit") ?? false;

            Assert.True(hasInitOnly, $"{prop.Name} must have init-only accessor");
        }
    }

    [Fact]
    public void NullHandling_ShouldHandleAllNullableFields()
    {
        var config = new KeywordConfig
        {
            Algorithm = null,
            MaxKeywords = null,
            MinScore = null,
            Language = null,
            NgramRange = null,
            YakeParams = null,
            RakeParams = null
        };

        Assert.Null(config.Algorithm);
        Assert.Null(config.MaxKeywords);
        Assert.Null(config.MinScore);
        Assert.Null(config.Language);
        Assert.Null(config.NgramRange);
        Assert.Null(config.YakeParams);
        Assert.Null(config.RakeParams);
    }

    [Fact]
    public void Serialization_ShouldUseJsonPropertyNames()
    {
        var config = new KeywordConfig
        {
            Algorithm = "yake",
            MaxKeywords = 10,
            Language = "en"
        };
        var json = JsonSerializer.Serialize(config);

        Assert.Contains("\"algorithm\"", json);
        Assert.Contains("\"max_keywords\"", json);
        Assert.Contains("\"language\"", json);
        Assert.Contains("\"yake\"", json);
        Assert.Contains("\"en\"", json);
    }

    [Fact]
    public void CompleteConfiguration_WithAllFields()
    {
        var config = new KeywordConfig
        {
            Algorithm = "yake",
            MaxKeywords = 15,
            MinScore = 0.4,
            Language = "en",
            NgramRange = new List<int> { 1, 2 },
            YakeParams = new YakeParamsConfig { WindowSize = 5 }
        };

        var json = JsonSerializer.Serialize(config);
        var restored = JsonSerializer.Deserialize<KeywordConfig>(json);

        Assert.NotNull(restored);
        Assert.Equal("yake", restored.Algorithm);
        Assert.Equal(15, restored.MaxKeywords);
        Assert.Equal(0.4, restored.MinScore);
        Assert.Equal("en", restored.Language);
        Assert.NotNull(restored.NgramRange);
        Assert.NotNull(restored.YakeParams);
    }
}
