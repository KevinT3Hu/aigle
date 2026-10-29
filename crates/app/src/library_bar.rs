//! The bar above the grid: filter by rating, label, camera, lens, date and keyword, and
//! choose the sort order. The grid and the filmstrip show the same filtered view.

use gpui::{App, Entity, IntoElement, ParentElement, SharedString, Styled, div, prelude::*, px};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_component::{ActiveTheme, Sizable, h_flex};

use crate::grid::stars;
use crate::i18n::t;
use crate::library::{LABELS, Library};
use crate::view::{Filter, LabelFilter, RatingFilter, Sort, SortKey};

/// A menu item that changes the filter with `change`.
fn filter_item(
    lib: &Entity<Library>,
    label: impl Into<SharedString>,
    checked: bool,
    change: impl Fn(&mut Filter) + 'static,
) -> PopupMenuItem {
    let lib = lib.clone();
    PopupMenuItem::new(label)
        .checked(checked)
        .on_click(move |_, _, cx| {
            lib.update(cx, |l, cx| {
                let mut f = l.filter().clone();
                change(&mut f);
                l.set_filter(f, cx);
            })
        })
}

fn sort_item(lib: &Entity<Library>, label: String, checked: bool, sort: Sort) -> PopupMenuItem {
    let lib = lib.clone();
    PopupMenuItem::new(label)
        .checked(checked)
        .on_click(move |_, _, cx| lib.update(cx, |l, cx| l.set_sort(sort, cx)))
}

fn rating_text(r: &RatingFilter) -> String {
    match r {
        RatingFilter::Any => t("library.filter.any"),
        RatingFilter::AtLeast(n) => format!("{}+", stars(*n)),
        RatingFilter::Unrated => t("library.filter.unrated"),
        RatingFilter::Rejected => t("library.filter.rejected"),
    }
}

fn label_text(l: &LabelFilter) -> String {
    match l {
        LabelFilter::Any => t("library.filter.any"),
        LabelFilter::None => t("library.filter.no_label"),
        LabelFilter::Is(name) => label_name(name),
    }
}

/// The translated name of a color label, or the label itself.
pub fn label_name(label: &str) -> String {
    let key = match label.to_lowercase().as_str() {
        "red" => "library.label.red",
        "yellow" => "library.label.yellow",
        "green" => "library.label.green",
        "blue" => "library.label.blue",
        "purple" => "library.label.purple",
        _ => return label.to_string(),
    };
    t(key)
}

/// `Rating` while unfiltered, `Rating: ★★★+` once set.
fn caption(category: &str, value: Option<String>) -> String {
    match value {
        Some(v) => format!("{}: {v}", t(category)),
        None => t(category),
    }
}

/// A dropdown choosing one of `values` (or any) for an optional filter field.
fn choice_menu(
    menu: PopupMenu,
    lib: &Entity<Library>,
    current: Option<String>,
    values: Vec<String>,
    field: fn(&mut Filter) -> &mut Option<String>,
) -> PopupMenu {
    let menu = menu
        .scrollable(true)
        .max_h(px(420.0))
        .item(filter_item(
            lib,
            t("library.filter.any"),
            current.is_none(),
            move |f| *field(f) = None,
        ))
        .separator();
    if values.is_empty() {
        return menu.label(t("library.filter.nothing"));
    }
    values.into_iter().fold(menu, |menu, v| {
        let checked = current.as_deref() == Some(v.as_str());
        let value = v.clone();
        menu.item(filter_item(lib, v, checked, move |f| {
            *field(f) = Some(value.clone())
        }))
    })
}

pub fn filter_bar(lib: &Entity<Library>, cx: &App) -> impl IntoElement + use<> {
    let (filter, sort) = {
        let l = lib.read(cx);
        (l.filter().clone(), l.sort())
    };
    let active = filter.is_active();

    let rating = {
        let lib = lib.clone();
        let current = filter.rating.clone();
        Button::new("filter-rating")
            .label(caption(
                "library.filter.rating",
                (current != RatingFilter::Any).then(|| rating_text(&current)),
            ))
            .xsmall()
            .ghost()
            .dropdown_menu(move |menu, _, _| {
                let mut options = vec![RatingFilter::Any];
                options.extend((1..=5).map(RatingFilter::AtLeast));
                options.extend([RatingFilter::Unrated, RatingFilter::Rejected]);
                options.into_iter().fold(menu, |menu, r| {
                    let checked = r == current;
                    let text = rating_text(&r);
                    menu.item(filter_item(&lib, text, checked, move |f| {
                        f.rating = r.clone()
                    }))
                })
            })
    };

    let label = {
        let lib = lib.clone();
        let current = filter.label.clone();
        Button::new("filter-label")
            .label(caption(
                "library.filter.label",
                (current != LabelFilter::Any).then(|| label_text(&current)),
            ))
            .xsmall()
            .ghost()
            .dropdown_menu(move |menu, _, _| {
                let mut options = vec![LabelFilter::Any, LabelFilter::None];
                options.extend(
                    LABELS
                        .iter()
                        .map(|(n, _)| *n)
                        .chain(["Purple"])
                        .map(|n| LabelFilter::Is(n.to_string())),
                );
                options.into_iter().fold(menu, |menu, o| {
                    let checked = o == current;
                    let text = label_text(&o);
                    menu.item(filter_item(&lib, text, checked, move |f| {
                        f.label = o.clone()
                    }))
                })
            })
    };

    let choice = |id: &'static str,
                  category: &'static str,
                  current: Option<String>,
                  values: fn(&Library) -> Vec<String>,
                  field: fn(&mut Filter) -> &mut Option<String>| {
        let lib = lib.clone();
        Button::new(id)
            .label(caption(category, current.clone()))
            .xsmall()
            .ghost()
            .dropdown_menu(move |menu, _, cx| {
                let values = values(lib.read(cx));
                choice_menu(menu, &lib, current.clone(), values, field)
            })
    };
    let camera = choice(
        "filter-camera",
        "library.filter.camera",
        filter.camera.clone(),
        |l| l.facets().cameras,
        |f| &mut f.camera,
    );
    let lens = choice(
        "filter-lens",
        "library.filter.lens",
        filter.lens.clone(),
        |l| l.facets().lenses,
        |f| &mut f.lens,
    );
    let keyword = choice(
        "filter-keyword",
        "library.filter.keyword",
        filter.keyword.clone(),
        |l| l.facets().keywords,
        |f| &mut f.keyword,
    );

    let date = {
        let lib = lib.clone();
        let current = filter.date.clone();
        Button::new("filter-date")
            .label(caption("library.filter.date", current.clone()))
            .xsmall()
            .ghost()
            .dropdown_menu(move |menu, window, cx| {
                let years = lib.read(cx).facets().years();
                let menu = menu
                    .item(filter_item(
                        &lib,
                        t("library.filter.any"),
                        current.is_none(),
                        |f| f.date = None,
                    ))
                    .separator();
                if years.is_empty() {
                    return menu.label(t("library.filter.nothing"));
                }
                years.into_iter().fold(menu, |menu, (year, days)| {
                    let lib = lib.clone();
                    let current = current.clone();
                    menu.submenu(year.clone(), window, cx, move |menu, _, _| {
                        let all =
                            rust_i18n::t!("library.filter.whole_year", arg0 = year).into_owned();
                        let y = year.clone();
                        let menu = menu
                            .scrollable(true)
                            .max_h(px(420.0))
                            .item(filter_item(
                                &lib,
                                all,
                                current.as_deref() == Some(year.as_str()),
                                move |f| f.date = Some(y.clone()),
                            ))
                            .separator();
                        days.iter().fold(menu, |menu, d| {
                            let day = d.clone();
                            menu.item(filter_item(
                                &lib,
                                d.clone(),
                                current.as_deref() == Some(d.as_str()),
                                move |f| f.date = Some(day.clone()),
                            ))
                        })
                    })
                })
            })
    };

    let sort_button = {
        let lib = lib.clone();
        let label = format!(
            "{} {}",
            caption("library.sort.title", Some(t(sort.key.label()))),
            if sort.descending { "↓" } else { "↑" }
        );
        Button::new("sort")
            .label(label)
            .xsmall()
            .ghost()
            .dropdown_menu(move |menu, _, _| {
                let menu = SortKey::ALL.into_iter().fold(menu, |menu, key| {
                    let next = Sort { key, ..sort };
                    menu.item(sort_item(&lib, t(key.label()), sort.key == key, next))
                });
                menu.separator()
                    .item(sort_item(
                        &lib,
                        t("library.sort.ascending"),
                        !sort.descending,
                        Sort {
                            descending: false,
                            ..sort
                        },
                    ))
                    .item(sort_item(
                        &lib,
                        t("library.sort.descending"),
                        sort.descending,
                        Sort {
                            descending: true,
                            ..sort
                        },
                    ))
            })
    };

    let clear = {
        let lib = lib.clone();
        Button::new("filter-clear")
            .label(t("library.filter.clear"))
            .xsmall()
            .ghost()
            .on_click(move |_, _, cx| lib.update(cx, |l, cx| l.set_filter(Filter::default(), cx)))
    };

    h_flex()
        .w_full()
        .flex_none()
        .px_2()
        .py_1()
        .gap_1()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(t("library.filter.title")),
        )
        .child(rating)
        .child(label)
        .child(camera)
        .child(lens)
        .child(date)
        .child(keyword)
        .when(active, |d| d.child(clear))
        .child(div().flex_1())
        .child(sort_button)
}
