//! Lightweight content-language detection. Used to route language-specific NLP
//! (sentiment, valence, extraction, personality, handoff atoms) to the right
//! lexicon instead of brute-forcing every supported language. Returns an ISO
//! 639-1 code, defaulting to "en" for short or low-confidence input where
//! guessing would do more harm than good.
//!
//! Kept deliberately free of any `crate::lexicon` dependency so it does not
//! conflict with concurrent lexicon work; the supported code set below must be
//! kept in step with the lexicon's embedded baselines (en/fr/de).

/// Convert the `whatlang` ISO 639-3 enum into the ISO 639-1 codes Kleos stores.
/// Unknown or unsupported languages return `None` so callers fall back to "en".
fn iso639_1(lang: whatlang::Lang) -> Option<&'static str> {
    match lang {
        whatlang::Lang::Deu => Some("de"),
        whatlang::Lang::Eng => Some("en"),
        whatlang::Lang::Fra => Some("fr"),
        _ => None,
    }
}

/// Detect the dominant language of `text`. Returns a 2-letter ISO 639-1 code,
/// falling back to "en" when the text is too short or detection is unconfident.
/// Never panics, so callers may use it on any ingest path without guarding.
pub fn detect_lang(text: &str) -> String {
    // Short strings are statistically unreliable; do not guess.
    if text.trim().chars().count() < 20 {
        return "en".to_string();
    }
    match whatlang::detect(text) {
        Some(info) if info.is_reliable() => iso639_1(info.lang()).unwrap_or("en").to_string(),
        _ => "en".to_string(),
    }
}

/// Unit tests for content-language detection.
#[cfg(test)]
mod tests {
    use super::*;

    /// Detection returns the right ISO 639-1 code for each supported language
    /// and defaults to "en" for input too short to classify.
    #[test]
    fn detects_supported_languages() {
        assert_eq!(
            detect_lang(
                "Die Geschwindigkeitsbegrenzung auf der Straße beträgt fünfzig Stundenkilometer."
            ),
            "de"
        );
        assert_eq!(
            detect_lang(
                "The quick brown fox jumps over the lazy dog repeatedly every morning today."
            ),
            "en"
        );
        assert_eq!(
            detect_lang(
                "La vitesse autorisée sur cette route nationale est de cinquante kilomètres heure."
            ),
            "fr"
        );
        // Too short to classify reliably -> default.
        assert_eq!(detect_lang("hi"), "en");
    }

    /// An unsupported language (not in the en/fr/de set) falls back to "en"
    /// rather than returning a code no lexicon can serve.
    #[test]
    fn unsupported_language_falls_back_to_en() {
        // Spanish is detectable by whatlang but has no Kleos lexicon yet.
        assert_eq!(
            detect_lang("La velocidad máxima permitida en esta carretera es de cincuenta kilómetros por hora."),
            "en"
        );
    }
}
