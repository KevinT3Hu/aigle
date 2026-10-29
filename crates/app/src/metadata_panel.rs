//! Keywords and IPTC fields of the selected photos: the library's side panel, and the
//! develop panel's Metadata tab (for the open photo only).
//!
//! With several photos selected a field shows the value they share, or nothing with a
//! "mixed" placeholder. A field is written to every selected photo when it is changed and
//! left (Enter or focus elsewhere). Keywords show the ones all selected photos have;
//! adding or removing one changes only that keyword on each photo. A keyword with levels
//! (`Places > France > Paris`) is stored as a Lightroom hierarchical keyword.

use std::collections::HashSet;

use aigle_library::{Field, PhotoKey, PhotoMeta, clean_keywords, normalize_tag};
use gpui::{
    Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription, Window, div,
    prelude::*, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Sizable, v_flex};

use crate::library::Library;

/// Separates keywords as typed and shown.
const KEYWORD_SEP: &str = ", ";

/// What an input shows for the photos it was filled from.
#[derive(Clone, Debug, Default, PartialEq)]
struct Shown {
    value: String,
    /// The photos disagree; `value` is what they share (keywords) or empty.
    mixed: bool,
}

struct Row {
    /// `None` for keywords.
    field: Option<Field>,
    input: Entity<InputState>,
    shown: Shown,
}

pub struct MetadataPanel {
    library: Entity<Library>,
    /// Only the primary photo (develop), shown inside a panel that scrolls.
    primary_only: bool,
    /// The photos the inputs were filled from.
    keys: Vec<PhotoKey>,
    rows: Vec<Row>,
    _subs: Vec<Subscription>,
}

fn split_keywords(s: &str) -> Vec<String> {
    clean_keywords(s.split([',', ';']).filter_map(normalize_tag))
}

/// The keywords (as [`PhotoMeta::tags`]) every one of `metas` has, in the first one's order.
fn common_keywords(metas: &[PhotoMeta]) -> Shown {
    let Some(first) = metas.first() else {
        return Shown::default();
    };
    let sets: Vec<HashSet<String>> = metas
        .iter()
        .map(|m| m.tags().iter().map(|k| k.to_lowercase()).collect())
        .collect();
    let common: Vec<String> = first
        .tags()
        .into_iter()
        .filter(|k| sets.iter().all(|s| s.contains(&k.to_lowercase())))
        .collect();
    Shown {
        mixed: sets.iter().any(|s| s.len() != common.len()),
        value: common.join(KEYWORD_SEP),
    }
}

fn common_field(metas: &[PhotoMeta], f: Field) -> Shown {
    let mut values = metas.iter().map(|m| m.field(f));
    let first = values.next().unwrap_or("");
    if values.all(|v| v == first) {
        Shown {
            value: first.to_string(),
            mixed: false,
        }
    } else {
        Shown {
            value: String::new(),
            mixed: true,
        }
    }
}

impl MetadataPanel {
    pub fn new(
        library: Entity<Library>,
        primary_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subs = vec![cx.observe_in(&library, window, |p, _, window, cx| p.sync(window, cx))];
        let fields = std::iter::once(None).chain(Field::ALL.into_iter().map(Some));
        let rows = fields
            .map(|field| {
                let input = cx.new(|cx| InputState::new(window, cx));
                subs.push(cx.subscribe_in(
                    &input,
                    window,
                    move |p, _, ev: &InputEvent, window, cx| {
                        if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                            p.commit(window, cx);
                        }
                    },
                ));
                Row {
                    field,
                    input,
                    shown: Shown::default(),
                }
            })
            .collect();
        let mut panel = MetadataPanel {
            library,
            primary_only,
            keys: Vec::new(),
            rows,
            _subs: subs,
        };
        panel.sync(window, cx);
        panel
    }

    /// The metadata of `keys` read so far (a photo whose sidecar is still being read counts
    /// as empty until it arrives).
    fn metas(&self, keys: &[PhotoKey], cx: &mut Context<Self>) -> Vec<PhotoMeta> {
        let l = self.library.read(cx);
        keys.iter()
            .filter_map(|k| l.item(l.index_of(k)?))
            .map(|it| it.meta.clone().unwrap_or_default())
            .collect()
    }

    /// Follow the selection and changes made elsewhere, keeping what is being typed.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keys: Vec<PhotoKey> = {
            let l = self.library.read(cx);
            let indices = if self.primary_only {
                l.selected.into_iter().collect()
            } else {
                l.selection()
            };
            indices
                .into_iter()
                .filter_map(|i| l.item(i).map(|it| it.key()))
                .collect()
        };
        let moved = keys != self.keys;
        if moved {
            self.commit(window, cx);
            self.keys = keys;
        }
        self.fill(moved, window, cx);
    }

    /// Show the stored values of `self.keys`. Unless `all`, an input whose text the user
    /// changed is left alone.
    fn fill(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        let metas = self.metas(&self.keys.clone(), cx);
        for row in &mut self.rows {
            let shown = match row.field {
                None => common_keywords(&metas),
                Some(f) => common_field(&metas, f),
            };
            let typed = row.input.read(cx).value().to_string();
            if !all && (shown == row.shown || typed != row.shown.value) {
                continue;
            }
            let placeholder = if shown.mixed {
                crate::i18n::t("metadata.mixed")
            } else {
                String::new()
            };
            row.input.update(cx, |i, cx| {
                i.set_value(shown.value.clone(), window, cx);
                i.set_placeholder(placeholder, window, cx);
            });
            row.shown = shown;
        }
        cx.notify();
    }

    /// Write every changed input to the photos it was filled from.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keys = self.keys.clone();
        let mut changed = false;
        for row in &mut self.rows {
            let typed = row.input.read(cx).value().to_string();
            if typed.trim() == row.shown.value.trim() {
                continue;
            }
            changed = true;
            self.library.update(cx, |l, cx| {
                let targets: Vec<usize> = keys.iter().filter_map(|k| l.index_of(k)).collect();
                match row.field {
                    Some(f) => {
                        for i in targets {
                            l.set_field(i, f, &typed, cx);
                        }
                    }
                    None => {
                        let before = split_keywords(&row.shown.value);
                        let after = split_keywords(&typed);
                        let lower = |v: &[String]| -> HashSet<String> {
                            v.iter().map(|k| k.to_lowercase()).collect()
                        };
                        let (was, now) = (lower(&before), lower(&after));
                        let add: Vec<String> = after
                            .iter()
                            .filter(|k| !was.contains(&k.to_lowercase()))
                            .cloned()
                            .collect();
                        let remove: Vec<String> = before
                            .iter()
                            .filter(|k| !now.contains(&k.to_lowercase()))
                            .cloned()
                            .collect();
                        for i in targets {
                            l.edit_keywords(i, &add, &remove, cx);
                        }
                    }
                }
            });
        }
        if changed {
            // Show the stored, cleaned values.
            self.fill(true, window, cx);
        }
    }
}

fn label_key(field: Option<Field>) -> String {
    match field {
        None => "metadata.keywords".to_string(),
        Some(f) => format!("metadata.field.{}", f.name()),
    }
}

impl Render for MetadataPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let n = self.keys.len();
        let header = match n {
            0 => crate::i18n::t("metadata.none_selected"),
            1 => crate::i18n::t("metadata.title"),
            n => rust_i18n::t!("metadata.title_many", arg0 = n).into_owned(),
        };
        let embedded = self.primary_only;
        v_flex()
            .id("metadata-panel")
            .w_full()
            .p_3()
            .gap_2()
            .when(!embedded, |d| d.h_full().overflow_y_scroll())
            .when(!embedded || n == 0, |d| {
                d.child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(header),
                )
            })
            .when(n > 0, |d| {
                d.children(self.rows.iter().map(|row| {
                    v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(crate::i18n::t(&label_key(row.field))),
                        )
                        .child(Input::new(&row.input).small())
                }))
                .child(
                    div()
                        .pt_1()
                        .text_xs()
                        .text_color(muted)
                        .max_w(px(300.0))
                        .child(crate::i18n::t("metadata.hint")),
                )
            })
    }
}

/// Keys of every label the panel shows.
#[cfg(test)]
pub fn label_keys() -> Vec<String> {
    std::iter::once(None)
        .chain(Field::ALL.into_iter().map(Some))
        .map(label_key)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(keywords: &[&str], title: &str) -> PhotoMeta {
        let mut m = PhotoMeta {
            keywords: keywords.iter().map(|k| k.to_string()).collect(),
            ..PhotoMeta::default()
        };
        m.set_field(Field::Title, title);
        m
    }

    #[test]
    fn shared_values() {
        let a = meta(&["sea", "Boat"], "Harbour");
        let b = meta(&["boat", "sea", "night"], "Harbour");
        assert_eq!(
            common_keywords(&[a.clone(), b.clone()]),
            Shown {
                value: "sea, Boat".into(),
                mixed: true
            }
        );
        assert!(!common_keywords(std::slice::from_ref(&a)).mixed);
        assert_eq!(common_field(&[a.clone(), b], Field::Title).value, "Harbour");
        let c = meta(&[], "Dock");
        assert!(common_field(&[a, c], Field::Title).mixed);
        assert_eq!(split_keywords("a, b;c ,, A"), ["a", "b", "c"]);
        assert_eq!(split_keywords("Places>France, x"), ["Places > France", "x"]);

        let mut h = PhotoMeta::default();
        h.set_tags(["Places > France", "sea"]);
        let mut i = PhotoMeta::default();
        i.set_tags(["places > france"]);
        assert_eq!(
            common_keywords(&[h, i]),
            Shown {
                value: "Places > France".into(),
                mixed: true
            }
        );
    }
}
