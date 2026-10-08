//! Ordered media inputs. Duration belongs to an image occurrence, not its file.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MediaItem {
    Video { path: PathBuf },
    Image { path: PathBuf, duration: f64 },
}

impl MediaItem {
    pub fn path(&self) -> &PathBuf {
        match self {
            Self::Video { path } | Self::Image { path, .. } => path,
        }
    }
    pub fn path_mut(&mut self) -> &mut PathBuf {
        match self {
            Self::Video { path } | Self::Image { path, .. } => path,
        }
    }
    pub fn is_image(&self) -> bool {
        matches!(self, Self::Image { .. })
    }
}
impl From<PathBuf> for MediaItem {
    fn from(path: PathBuf) -> Self {
        Self::Video { path }
    }
}
impl From<&str> for MediaItem {
    fn from(path: &str) -> Self {
        PathBuf::from(path).into()
    }
}

/// Preserve the original video-only JSON while normalizing both input forms.
/// The domain model always carries exactly one ordered collection.
pub(crate) mod item_list {
    use super::*;
    use serde::{Deserializer, Serializer, de::Error, ser::SerializeMap};

    fn present<'de, D, T>(deserializer: D) -> Result<Option<Vec<T>>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        Vec::deserialize(deserializer).map(Some)
    }

    #[derive(Deserialize)]
    struct Wire {
        #[serde(default, deserialize_with = "present")]
        videos: Option<Vec<PathBuf>>,
        #[serde(default, deserialize_with = "present")]
        items: Option<Vec<MediaItem>>,
    }
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<MediaItem>, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Wire::deserialize(deserializer)? {
            Wire {
                videos: Some(paths),
                items: None,
            } => Ok(paths.into_iter().map(Into::into).collect()),
            Wire {
                videos: None,
                items: Some(items),
            } => Ok(items),
            _ => Err(D::Error::custom("Provide exactly one of videos or items")),
        }
    }
    pub fn serialize<S>(items: &[MediaItem], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(1))?;
        if items.iter().all(|item| !item.is_image()) {
            map.serialize_entry(
                "videos",
                &items.iter().map(MediaItem::path).collect::<Vec<_>>(),
            )?;
        } else {
            map.serialize_entry("items", items)?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Request {
        #[serde(flatten, with = "item_list")]
        items: Vec<MediaItem>,
        other: bool,
    }

    #[test]
    fn video_only_and_typed_inputs_share_one_ordered_model() {
        let old = json!({"videos":["a.mp4","b.mp4"],"other":true});
        let request: Request = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(request.items, vec!["a.mp4".into(), "b.mp4".into()]);
        assert_eq!(serde_json::to_value(&request).unwrap(), old);
        let typed = json!({"items":[{"kind":"video","path":"a.mp4"},{"kind":"video","path":"b.mp4"}],"other":true});
        assert_eq!(request, serde_json::from_value(typed).unwrap());
        let mixed = json!({"items":[{"kind":"image","path":"same.png","duration":1.0},{"kind":"video","path":"a.mp4"},{"kind":"image","path":"same.png","duration":2.0}],"other":false});
        let request: Request = serde_json::from_value(mixed.clone()).unwrap();
        assert_eq!(serde_json::to_value(&request).unwrap(), mixed);
        assert_ne!(request.items[0], request.items[2]);
    }

    #[test]
    fn ambiguous_missing_null_duplicate_and_unknown_item_fields_are_rejected() {
        for invalid in [
            r#"{"other":true}"#,
            r#"{"other":true,"videos":[],"items":[]}"#,
            r#"{"other":true,"videos":null}"#,
            r#"{"other":true,"items":null}"#,
            r#"{"other":true,"videos":null,"items":[]}"#,
            r#"{"other":true,"videos":[],"videos":[]}"#,
            r#"{"other":true,"items":[],"items":[]}"#,
            r#"{"other":true,"items":[{"kind":"video","path":"a","duration":1}]}"#,
            r#"{"other":true,"items":[{"kind":"image","path":"a"}]}"#,
        ] {
            assert!(
                serde_json::from_str::<Request>(invalid).is_err(),
                "{invalid}"
            );
        }
    }
}
