//! Which photos of the folder the grid and filmstrip show, and in what order.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use crate::library::Item;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RatingFilter {
    #[default]
    Any,
    /// This many stars or more (rejected photos are left out).
    AtLeast(i8),
    Unrated,
    Rejected,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LabelFilter {
    #[default]
    Any,
    None,
    /// `xmp:Label`, compared case-insensitively.
    Is(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub rating: RatingFilter,
    pub label: LabelFilter,
    pub camera: Option<String>,
    pub lens: Option<String>,
    /// A prefix of the capture time: `2024` or `2024-05-06`.
    pub date: Option<String>,
    pub keyword: Option<String>,
}

impl Filter {
    pub fn is_active(&self) -> bool {
        *self != Filter::default()
    }

    /// The filter for another folder: its cameras, lenses, dates and keywords differ.
    pub fn for_new_folder(&self) -> Filter {
        Filter {
            rating: self.rating.clone(),
            label: self.label.clone(),
            ..Filter::default()
        }
    }

    fn needs_facts(&self) -> bool {
        self.camera.is_some() || self.lens.is_some() || self.date.is_some()
    }

    fn needs_meta(&self) -> bool {
        self.rating != RatingFilter::Any || self.label != LabelFilter::Any || self.keyword.is_some()
    }

    /// Photos whose sidecar or EXIF has not been read yet do not match a filter that needs
    /// them.
    pub fn matches(&self, item: &Item) -> bool {
        if (self.needs_meta() && item.meta.is_none())
            || (self.needs_facts() && item.facts.is_none())
        {
            return false;
        }
        let rating = item.rating();
        let rating_ok = match self.rating {
            RatingFilter::Any => true,
            RatingFilter::AtLeast(n) => rating >= n,
            RatingFilter::Unrated => rating == 0,
            RatingFilter::Rejected => rating < 0,
        };
        let label_ok = match &self.label {
            LabelFilter::Any => true,
            LabelFilter::None => item.label().is_none(),
            LabelFilter::Is(l) => item.label().is_some_and(|x| x.eq_ignore_ascii_case(l)),
        };
        let facts = item.facts.as_ref();
        let camera_ok =
            self.camera.is_none() || facts.and_then(|f| f.camera.as_ref()) == self.camera.as_ref();
        let lens_ok =
            self.lens.is_none() || facts.and_then(|f| f.lens.as_ref()) == self.lens.as_ref();
        let date_ok = self.date.as_ref().is_none_or(|d| {
            facts
                .and_then(|f| f.captured.as_deref())
                .is_some_and(|c| c.starts_with(d.as_str()))
        });
        let keyword_ok = self
            .keyword
            .as_ref()
            .is_none_or(|k| item.meta.as_ref().is_some_and(|m| m.has_keyword(k)));
        rating_ok && label_ok && camera_ok && lens_ok && date_ok && keyword_ok
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKey {
    #[default]
    Name,
    Captured,
    Rating,
    Modified,
}

impl SortKey {
    pub const ALL: [SortKey; 4] = [
        SortKey::Name,
        SortKey::Captured,
        SortKey::Rating,
        SortKey::Modified,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "library.sort.name",
            SortKey::Captured => "library.sort.captured",
            SortKey::Rating => "library.sort.rating",
            SortKey::Modified => "library.sort.modified",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
}

/// `None` after every value, whichever the direction.
fn cmp_known<T: Ord>(a: Option<T>, b: Option<T>, descending: bool) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) if descending => b.cmp(&a),
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// File name, then folder, then the photo before its virtual copies (oldest first).
fn by_name(a: &Item, b: &Item) -> Ordering {
    a.entry
        .name
        .to_lowercase()
        .cmp(&b.entry.name.to_lowercase())
        .then_with(|| a.entry.name.cmp(&b.entry.name))
        .then_with(|| a.entry.path.cmp(&b.entry.path))
        .then_with(|| {
            a.copy
                .as_ref()
                .map(|c| c.id)
                .cmp(&b.copy.as_ref().map(|c| c.id))
        })
}

impl Sort {
    pub fn cmp(&self, a: &Item, b: &Item) -> Ordering {
        let d = self.descending;
        let primary = match self.key {
            SortKey::Name => {
                let o = by_name(a, b);
                return if d { o.reverse() } else { o };
            }
            SortKey::Captured => cmp_known(
                a.facts.as_ref().and_then(|f| f.captured.as_ref()),
                b.facts.as_ref().and_then(|f| f.captured.as_ref()),
                d,
            ),
            SortKey::Rating => cmp_known(
                a.meta.as_ref().map(|m| m.rating),
                b.meta.as_ref().map(|m| m.rating),
                d,
            ),
            SortKey::Modified => cmp_known(a.entry.modified, b.entry.modified, d),
        };
        primary.then_with(|| by_name(a, b))
    }

    /// The order changes as sidecars and EXIF are read.
    fn needs_loading(&self) -> bool {
        matches!(self.key, SortKey::Captured | SortKey::Rating)
    }
}

/// The view must be rebuilt when metadata or facts arrive.
pub fn depends_on_loading(filter: &Filter, sort: &Sort) -> bool {
    filter.needs_meta() || filter.needs_facts() || sort.needs_loading()
}

/// Values the filter menus offer, from every photo of the folder.
#[derive(Default)]
pub struct Facets {
    pub cameras: Vec<String>,
    pub lenses: Vec<String>,
    /// Capture days (`YYYY-MM-DD`), newest first.
    pub days: Vec<String>,
    pub keywords: Vec<String>,
}

impl Facets {
    pub fn of<'a>(items: impl IntoIterator<Item = &'a Item>) -> Facets {
        let (mut cameras, mut lenses, mut days) =
            (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        let mut keywords: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for it in items {
            if let Some(f) = &it.facts {
                cameras.extend(f.camera.clone());
                lenses.extend(f.lens.clone());
                days.extend(f.day().map(str::to_string));
            }
            for k in it.meta.iter().flat_map(|m| m.all_keywords()) {
                if seen.insert(k.to_lowercase()) {
                    keywords.push(k);
                }
            }
        }
        keywords.sort_by_key(|k| k.to_lowercase());
        Facets {
            cameras: cameras.into_iter().collect(),
            lenses: lenses.into_iter().collect(),
            days: days.into_iter().rev().collect(),
            keywords,
        }
    }

    /// Years of [`Facets::days`], newest first, with their days.
    pub fn years(&self) -> Vec<(String, Vec<String>)> {
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for d in &self.days {
            let y = &d[..4];
            match out.last_mut() {
                Some((year, days)) if year == y => days.push(d.clone()),
                _ => out.push((y.to_string(), vec![d.clone()])),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aigle_library::{Facts, PhotoMeta};

    fn item(name: &str, rating: i8, label: Option<&str>, captured: Option<&str>) -> Item {
        let meta = PhotoMeta {
            rating,
            label: label.map(str::to_string),
            keywords: vec!["Sea".into()],
            ..PhotoMeta::default()
        };
        let facts = Facts {
            camera: Some("Canon EOS 6D".into()),
            lens: None,
            captured: captured.map(str::to_string),
        };
        Item::test(name, None, Some(meta), Some(facts))
    }

    fn names(items: &[Item], filter: &Filter, sort: Sort) -> Vec<String> {
        let mut v: Vec<&Item> = items.iter().filter(|it| filter.matches(it)).collect();
        v.sort_by(|a, b| sort.cmp(a, b));
        v.into_iter().map(Item::name).collect()
    }

    #[test]
    fn filters_and_sorts() {
        let items = vec![
            item("b.nef", 3, Some("Red"), Some("2024-05-06 10:00:00")),
            item("A.nef", -1, None, None),
            item("c.nef", 5, None, Some("2023-01-01 09:00:00")),
            Item::test("b.nef", Some(7), Some(PhotoMeta::default()), None),
        ];
        let all = Filter::default();
        assert_eq!(
            names(&items, &all, Sort::default()),
            ["A.nef", "b.nef", "b (Copy 7).nef", "c.nef"]
        );
        let by_date = Sort {
            key: SortKey::Captured,
            descending: true,
        };
        assert_eq!(
            names(&items, &all, by_date),
            ["b.nef", "c.nef", "A.nef", "b (Copy 7).nef"]
        );
        let by_rating = Sort {
            key: SortKey::Rating,
            descending: false,
        };
        assert_eq!(
            names(&items, &all, by_rating),
            ["A.nef", "b (Copy 7).nef", "b.nef", "c.nef"]
        );

        let f = |change: fn(&mut Filter)| {
            let mut f = Filter::default();
            change(&mut f);
            names(&items, &f, Sort::default())
        };
        assert_eq!(
            f(|f| f.rating = RatingFilter::AtLeast(3)),
            ["b.nef", "c.nef"]
        );
        assert_eq!(f(|f| f.rating = RatingFilter::Rejected), ["A.nef"]);
        assert_eq!(f(|f| f.rating = RatingFilter::Unrated), ["b (Copy 7).nef"]);
        assert_eq!(f(|f| f.label = LabelFilter::Is("red".into())), ["b.nef"]);
        assert_eq!(f(|f| f.date = Some("2024".into())), ["b.nef"]);
        assert_eq!(
            f(|f| f.keyword = Some("sea".into())),
            ["A.nef", "b.nef", "c.nef"]
        );
        // The copy's EXIF has not been read yet.
        assert_eq!(
            f(|f| f.camera = Some("Canon EOS 6D".into())),
            ["A.nef", "b.nef", "c.nef"]
        );
    }

    #[test]
    fn facets_group_days_by_year() {
        let items = vec![
            item("a", 0, None, Some("2024-05-06 10:00:00")),
            item("b", 0, None, Some("2024-05-07 10:00:00")),
            item("c", 0, None, Some("2023-01-01 09:00:00")),
        ];
        let f = Facets::of(&items);
        assert_eq!(f.cameras, ["Canon EOS 6D"]);
        assert_eq!(f.keywords, ["Sea"]);
        assert_eq!(
            f.years(),
            [
                (
                    "2024".to_string(),
                    vec!["2024-05-07".to_string(), "2024-05-06".to_string()]
                ),
                ("2023".to_string(), vec!["2023-01-01".to_string()]),
            ]
        );
    }
}
