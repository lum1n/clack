use crate::error::{ClackError, ClackResult};

/// Turn a Hugging Face repo id or a model page URL into `org/model`.
///
/// Accepts `owner/name` and `http(s)` links on `huggingface.co` or `hf.co`.
/// Extra path (`/tree/main`, `/blob/...`, `/resolve/...`), query strings,
/// fragments, and a trailing slash are dropped. Dataset and Space URLs are rejected.
pub fn normalize_model_ref(input: &str) -> ClackResult<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(invalid(trimmed, "paste a repo id (org/model) or a Hugging Face model URL"));
    }

    let (path, from_url) = match split_url(trimmed)? {
        Some(path) => (path, true),
        None => (trimmed.to_string(), false),
    };
    let path = path.split(['?', '#']).next().unwrap_or("").trim_matches('/');
    if path.is_empty() {
        return Err(invalid(trimmed, "the link has no model path"));
    }
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.first().copied() == Some("datasets") || parts.first().copied() == Some("spaces") {
        return Err(invalid(
            trimmed,
            "that link is not a model repo (datasets and spaces are not loaded)",
        ));
    }

    if from_url {
        if parts.len() < 2 {
            return Err(invalid(trimmed, "expected a model URL like https://huggingface.co/org/model"));
        }
        return Ok(format!("{}/{}", parts[0], parts[1]));
    }

    if parts.len() == 2 && !parts[0].contains(':') && !parts[1].contains(':') {
        return Ok(format!("{}/{}", parts[0], parts[1]));
    }
    Err(invalid(
        trimmed,
        "expected org/model, or a huggingface.co / hf.co model URL",
    ))
}

fn invalid(input: &str, detail: &str) -> ClackError {
    ClackError::InvalidRepo {
        repo_id: input.to_string(),
        detail: detail.to_string(),
    }
}

fn split_url(input: &str) -> ClackResult<Option<String>> {
    let Some(rest) = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))
    else {
        if looks_like_host_without_scheme(input) {
            return strip_known_host(input).map(Some);
        }
        return Ok(None);
    };
    strip_known_host(rest).map(Some)
}

fn looks_like_host_without_scheme(input: &str) -> bool {
    let host = input.split('/').next().unwrap_or("");
    is_hf_host(host)
}

fn strip_known_host(rest: &str) -> ClackResult<String> {
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.split(['?', '#']).next().unwrap_or(host);
    if !is_hf_host(host) {
        return Err(invalid(
            rest,
            "only huggingface.co and hf.co model links are accepted",
        ));
    }
    Ok(path.to_string())
}

fn is_hf_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.');
    matches!(
        host,
        "huggingface.co" | "www.huggingface.co" | "hf.co" | "www.hf.co"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_id_passes_through() {
        assert_eq!(
            normalize_model_ref("  openai/whisper-tiny ").unwrap(),
            "openai/whisper-tiny"
        );
    }

    #[test]
    fn urls_drop_extra_path_query_and_slash() {
        let cases = [
            "https://huggingface.co/openai/whisper-tiny",
            "https://huggingface.co/openai/whisper-tiny/",
            "https://huggingface.co/openai/whisper-tiny/tree/main",
            "https://huggingface.co/openai/whisper-tiny/blob/main/config.json",
            "https://huggingface.co/openai/whisper-tiny/resolve/main/model.safetensors?download=true",
            "http://www.huggingface.co/openai/whisper-tiny#readme",
            "https://hf.co/openai/whisper-tiny/",
            "https://hf.co/openai/whisper-tiny?not-for-all-audiences=true",
            "hf.co/distil-whisper/distil-small.en/tree/main/",
        ];
        for case in cases {
            let got = normalize_model_ref(case).unwrap_or_else(|err| panic!("{case}: {err}"));
            assert!(
                got == "openai/whisper-tiny" || got == "distil-whisper/distil-small.en",
                "{case} -> {got}"
            );
        }
    }

    #[test]
    fn rejects_datasets_and_other_hosts() {
        assert!(normalize_model_ref("https://huggingface.co/datasets/foo/bar").is_err());
        assert!(normalize_model_ref("https://example.com/openai/whisper-tiny").is_err());
        assert!(normalize_model_ref("not a model").is_err());
        assert!(normalize_model_ref("openai/whisper-tiny/tree/main").is_err());
    }
}
