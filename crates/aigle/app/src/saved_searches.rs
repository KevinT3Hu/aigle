//! Saved library filters apply to the current folder, without changing originals.
use crate::view::{Filter, Sort};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSearch {
    pub name: String,
    pub filter: Filter,
    pub sort: Sort,
}

pub fn load() -> anyhow::Result<Vec<SavedSearch>> {
    let Some(dir) = crate::config_dir() else {
        return Ok(Vec::new());
    };
    let path = dir.join("saved-searches.json");
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

pub fn save(searches: &[SavedSearch]) -> anyhow::Result<()> {
    let dir = crate::config_dir().ok_or_else(|| anyhow::anyhow!("No configuration directory"))?;
    std::fs::create_dir_all(&dir)?;
    let temp = dir.join("saved-searches.json.tmp");
    let mut file = std::fs::File::create(&temp)?;
    use std::io::Write;
    file.write_all(&serde_json::to_vec_pretty(searches)?)?;
    file.sync_all()?;
    std::fs::rename(temp, dir.join("saved-searches.json"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_search_preserves_all_criteria() {
        let search = SavedSearch {
            name: "Five star beaches".into(),
            filter: Filter {
                query: "beach -portrait".into(),
                rating: crate::view::RatingFilter::AtLeast(5),
                ..Default::default()
            },
            sort: Sort {
                key: crate::view::SortKey::Captured,
                descending: true,
            },
        };
        let decoded: SavedSearch =
            serde_json::from_slice(&serde_json::to_vec(&search).unwrap()).unwrap();
        assert_eq!(search, decoded);
    }
}
