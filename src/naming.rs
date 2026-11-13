use std::path::{Component, Path};

use anyhow::Result;

pub fn validate_subdir(path: &Path) -> Result<()> {
    anyhow::ensure!(
        path.components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "--dest-subdir must be a relative path without '..'"
    );
    Ok(())
}

pub fn flatten(path: &Path, max_length: usize, suffix: Option<&str>) -> String {
    let joined = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(value) => Some(sanitize(&value.to_string_lossy())),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("_");
    let (stem, extension) = match joined.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => (stem, format!(".{ext}")),
        _ => (joined.as_str(), String::new()),
    };
    let suffix = suffix.map(|s| format!("_{s}")).unwrap_or_default();
    // Reserve room for a valid stem and the collision suffix even for huge extensions.
    let extension: String = extension
        .chars()
        .take(max_length.saturating_sub(suffix.len() + 2))
        .collect();
    let room = max_length - suffix.len() - extension.chars().count();
    let stem = tail(stem, room);
    let stem = sanitize(&stem);
    let stem = if reserved(&stem) || stem.chars().count() > room {
        format!("_{}", tail(&stem, room.saturating_sub(1)))
    } else {
        stem
    };
    format!("{stem}{suffix}{extension}")
}

fn tail(value: &str, count: usize) -> String {
    value
        .chars()
        .skip(value.chars().count().saturating_sub(count))
        .collect()
}

fn sanitize(value: &str) -> String {
    let value: String = value
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = value.trim_end_matches([' ', '.']);
    let value = if trimmed.is_empty() { "_" } else { trimmed };
    if reserved(value) {
        format!("_{value}")
    } else {
        value.to_owned()
    }
}

fn reserved(value: &str) -> bool {
    let stem = value
        .split('.')
        .next()
        .unwrap_or(value)
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|number| {
                matches!(
                    number,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_paths_and_reserved_names() {
        assert_eq!(
            flatten(
                Path::new("2023").join("trip").join("a.jpg").as_path(),
                64,
                None
            ),
            "2023_trip_a.jpg"
        );
        assert_eq!(flatten(Path::new("CON.txt"), 64, None), "_CON.txt");
        assert_eq!(sanitize("a:b?. "), "a_b_");
        assert!(validate_subdir(Path::new("..").join("outside").as_path()).is_err());
    }

    #[test]
    fn truncation_preserves_extension_suffix_and_unicode() {
        let name = flatten(
            Path::new("very_long_parent_éééééééééééé.jpg"),
            24,
            Some("abcdef"),
        );
        assert_eq!(name.chars().count(), 24);
        assert!(name.ends_with("_abcdef.jpg"));
        assert!(!name.starts_with("very"));
        assert!(
            flatten(
                Path::new("name.abcdefghijklmnopqrstuvwxyz"),
                16,
                Some("abcdef")
            )
            .chars()
            .count()
                <= 16
        );
    }
}
