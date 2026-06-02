use std::path::Path;

pub(crate) fn sanitized_basename(path_or_filename: &str) -> Option<String> {
    let file_name = Path::new(path_or_filename).file_name()?.to_str()?;
    if file_name.is_empty() {
        return None;
    }
    Some(file_name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitized_basename_accepts_plain_filename() {
        assert_eq!(
            sanitized_basename("report.txt"),
            Some("report.txt".to_string())
        );
    }

    #[test]
    fn sanitized_basename_extracts_from_path() {
        assert_eq!(
            sanitized_basename("outputs/report.txt"),
            Some("report.txt".to_string())
        );
    }
}
