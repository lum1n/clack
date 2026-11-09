use crate::error::{ClackError, ClackResult};

/// Output language for the text that gets produced.
///
/// `Auto` lets the model detect the spoken language and transcribe in it.
/// `Code` forces that language on the decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanguageMode {
    Auto,
    Code(String),
}

impl LanguageMode {
    pub fn as_code(&self) -> &str {
        match self {
            Self::Auto => "auto",
            Self::Code(code) => code,
        }
    }

    pub fn is_english(&self) -> bool {
        matches!(self, Self::Code(code) if code == "en")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageOption {
    pub code: &'static str,
    pub name: &'static str,
}

/// Auto first, then Whisper's language list (name plus code).
pub fn output_languages() -> &'static [LanguageOption] {
    &LANGUAGES
}

pub fn parse_language(raw: &str) -> ClackResult<LanguageMode> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ClackError::InvalidLanguage {
            language: raw.to_string(),
            detail: "expected \"auto\" or a language code such as \"en\"".into(),
        });
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower == "auto" || lower == "detect" {
        return Ok(LanguageMode::Auto);
    }
    if let Some(option) = LANGUAGES.iter().find(|item| item.code == lower) {
        return Ok(LanguageMode::Code(option.code.to_string()));
    }
    if let Some(option) = LANGUAGES
        .iter()
        .find(|item| item.name.eq_ignore_ascii_case(trimmed))
    {
        return Ok(LanguageMode::Code(option.code.to_string()));
    }
    Err(ClackError::InvalidLanguage {
        language: trimmed.to_string(),
        detail: "not \"auto\" and not a Whisper language code or name".into(),
    })
}

pub fn language_label(code: &str) -> String {
    if code.eq_ignore_ascii_case("auto") {
        return "Auto (auto)".into();
    }
    LANGUAGES
        .iter()
        .find(|item| item.code == code)
        .map(|item| format!("{} ({})", item.name, item.code))
        .unwrap_or_else(|| code.to_string())
}

/// Whisper's multilingual token set. Index 0 is Auto.
const LANGUAGES: &[LanguageOption] = &[
    LanguageOption { code: "auto", name: "Auto" },
    LanguageOption { code: "en", name: "English" },
    LanguageOption { code: "zh", name: "Chinese" },
    LanguageOption { code: "de", name: "German" },
    LanguageOption { code: "es", name: "Spanish" },
    LanguageOption { code: "ru", name: "Russian" },
    LanguageOption { code: "ko", name: "Korean" },
    LanguageOption { code: "fr", name: "French" },
    LanguageOption { code: "ja", name: "Japanese" },
    LanguageOption { code: "pt", name: "Portuguese" },
    LanguageOption { code: "tr", name: "Turkish" },
    LanguageOption { code: "pl", name: "Polish" },
    LanguageOption { code: "ca", name: "Catalan" },
    LanguageOption { code: "nl", name: "Dutch" },
    LanguageOption { code: "ar", name: "Arabic" },
    LanguageOption { code: "sv", name: "Swedish" },
    LanguageOption { code: "it", name: "Italian" },
    LanguageOption { code: "id", name: "Indonesian" },
    LanguageOption { code: "hi", name: "Hindi" },
    LanguageOption { code: "fi", name: "Finnish" },
    LanguageOption { code: "vi", name: "Vietnamese" },
    LanguageOption { code: "he", name: "Hebrew" },
    LanguageOption { code: "uk", name: "Ukrainian" },
    LanguageOption { code: "el", name: "Greek" },
    LanguageOption { code: "ms", name: "Malay" },
    LanguageOption { code: "cs", name: "Czech" },
    LanguageOption { code: "ro", name: "Romanian" },
    LanguageOption { code: "da", name: "Danish" },
    LanguageOption { code: "hu", name: "Hungarian" },
    LanguageOption { code: "ta", name: "Tamil" },
    LanguageOption { code: "no", name: "Norwegian" },
    LanguageOption { code: "th", name: "Thai" },
    LanguageOption { code: "ur", name: "Urdu" },
    LanguageOption { code: "hr", name: "Croatian" },
    LanguageOption { code: "bg", name: "Bulgarian" },
    LanguageOption { code: "lt", name: "Lithuanian" },
    LanguageOption { code: "la", name: "Latin" },
    LanguageOption { code: "mi", name: "Maori" },
    LanguageOption { code: "ml", name: "Malayalam" },
    LanguageOption { code: "cy", name: "Welsh" },
    LanguageOption { code: "sk", name: "Slovak" },
    LanguageOption { code: "te", name: "Telugu" },
    LanguageOption { code: "fa", name: "Persian" },
    LanguageOption { code: "lv", name: "Latvian" },
    LanguageOption { code: "bn", name: "Bengali" },
    LanguageOption { code: "sr", name: "Serbian" },
    LanguageOption { code: "az", name: "Azerbaijani" },
    LanguageOption { code: "sl", name: "Slovenian" },
    LanguageOption { code: "kn", name: "Kannada" },
    LanguageOption { code: "et", name: "Estonian" },
    LanguageOption { code: "mk", name: "Macedonian" },
    LanguageOption { code: "br", name: "Breton" },
    LanguageOption { code: "eu", name: "Basque" },
    LanguageOption { code: "is", name: "Icelandic" },
    LanguageOption { code: "hy", name: "Armenian" },
    LanguageOption { code: "ne", name: "Nepali" },
    LanguageOption { code: "mn", name: "Mongolian" },
    LanguageOption { code: "bs", name: "Bosnian" },
    LanguageOption { code: "kk", name: "Kazakh" },
    LanguageOption { code: "sq", name: "Albanian" },
    LanguageOption { code: "sw", name: "Swahili" },
    LanguageOption { code: "gl", name: "Galician" },
    LanguageOption { code: "mr", name: "Marathi" },
    LanguageOption { code: "pa", name: "Punjabi" },
    LanguageOption { code: "si", name: "Sinhala" },
    LanguageOption { code: "km", name: "Khmer" },
    LanguageOption { code: "sn", name: "Shona" },
    LanguageOption { code: "yo", name: "Yoruba" },
    LanguageOption { code: "so", name: "Somali" },
    LanguageOption { code: "af", name: "Afrikaans" },
    LanguageOption { code: "oc", name: "Occitan" },
    LanguageOption { code: "ka", name: "Georgian" },
    LanguageOption { code: "be", name: "Belarusian" },
    LanguageOption { code: "tg", name: "Tajik" },
    LanguageOption { code: "sd", name: "Sindhi" },
    LanguageOption { code: "gu", name: "Gujarati" },
    LanguageOption { code: "am", name: "Amharic" },
    LanguageOption { code: "yi", name: "Yiddish" },
    LanguageOption { code: "lo", name: "Lao" },
    LanguageOption { code: "uz", name: "Uzbek" },
    LanguageOption { code: "fo", name: "Faroese" },
    LanguageOption { code: "ht", name: "Haitian Creole" },
    LanguageOption { code: "ps", name: "Pashto" },
    LanguageOption { code: "tk", name: "Turkmen" },
    LanguageOption { code: "nn", name: "Nynorsk" },
    LanguageOption { code: "mt", name: "Maltese" },
    LanguageOption { code: "sa", name: "Sanskrit" },
    LanguageOption { code: "lb", name: "Luxembourgish" },
    LanguageOption { code: "my", name: "Myanmar" },
    LanguageOption { code: "bo", name: "Tibetan" },
    LanguageOption { code: "tl", name: "Tagalog" },
    LanguageOption { code: "mg", name: "Malagasy" },
    LanguageOption { code: "as", name: "Assamese" },
    LanguageOption { code: "tt", name: "Tatar" },
    LanguageOption { code: "haw", name: "Hawaiian" },
    LanguageOption { code: "ln", name: "Lingala" },
    LanguageOption { code: "ha", name: "Hausa" },
    LanguageOption { code: "ba", name: "Bashkir" },
    LanguageOption { code: "jw", name: "Javanese" },
    LanguageOption { code: "su", name: "Sundanese" },
    LanguageOption { code: "yue", name: "Cantonese" },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_is_first_and_names_have_codes() {
        let languages = output_languages();
        assert_eq!(languages[0].code, "auto");
        assert!(languages.iter().any(|item| item.code == "en" && item.name == "English"));
        assert!(languages.iter().any(|item| item.code == "fr"));
    }

    #[test]
    fn parse_accepts_auto_codes_and_names() {
        assert_eq!(parse_language("auto").unwrap(), LanguageMode::Auto);
        assert_eq!(parse_language(" EN ").unwrap().as_code(), "en");
        assert_eq!(parse_language("French").unwrap().as_code(), "fr");
        assert!(parse_language("not-a-language").is_err());
    }
}
