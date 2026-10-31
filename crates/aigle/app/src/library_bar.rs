//! The bar above the grid: filter by rating, label, camera, lens, date and keyword, list
//! subfolders too, and choose the sort order. The grid and the filmstrip show the same
//! filtered view.

use gpui::{App, Entity, IntoElement, ParentElement, SharedString, Styled, div, prelude::*, px};
use gpui_component::button::ButtonVariants;
use gpui_component::input::Input;
use gpui_component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_component::{ActiveTheme, Selectable, Sizable, h_flex};

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
        RatingFilter::Picked => t("library.filter.picked"),
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
        crate::ui::labeled_button(
            "filter-rating",
            crate::ui::Glyph::Star,
            caption(
                "library.filter.rating",
                (current != RatingFilter::Any).then(|| rating_text(&current)),
            ),
        )
        .xsmall()
        .ghost()
        .dropdown_menu(move |menu, _, _| {
            let mut options = vec![RatingFilter::Any];
            options.extend((1..=5).map(RatingFilter::AtLeast));
            options.extend([
                RatingFilter::Unrated,
                RatingFilter::Rejected,
                RatingFilter::Picked,
            ]);
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
        crate::ui::labeled_button(
            "filter-label",
            crate::ui::Glyph::Tag,
            caption(
                "library.filter.label",
                (current != LabelFilter::Any).then(|| label_text(&current)),
            ),
        )
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
        crate::ui::labeled_button(
            id,
            match id {
                "filter-camera" => crate::ui::Glyph::Camera,
                "filter-lens" => crate::ui::Glyph::Lens,
                _ => crate::ui::Glyph::Filter,
            },
            caption(category, current.clone()),
        )
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
        crate::ui::labeled_button(
            "filter-date",
            crate::ui::Glyph::Calendar,
            caption("library.filter.date", current.clone()),
        )
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
                    let all = rust_i18n::t!("library.filter.whole_year", arg0 = year).into_owned();
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
        crate::ui::labeled_button("sort", crate::ui::Glyph::Sort, label)
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
        crate::ui::icon_button(
            "filter-clear",
            crate::ui::Glyph::Clear,
            t("library.filter.clear"),
        )
        .xsmall()
        .ghost()
        .on_click(move |_, window, cx| {
            lib.update(cx, |l, cx| {
                l.apply_search(Filter::default(), l.sort(), window, cx)
            })
        })
    };

    let subfolders = {
        let lib = lib.clone();
        let on = lib.read(cx).recursive();
        crate::ui::icon_button(
            "subfolders",
            crate::ui::Glyph::Folder,
            t("action.ToggleSubfolders"),
        )
        .xsmall()
        .ghost()
        .selected(on)
        .tooltip(t("library.subfolders_tooltip"))
        .on_click(move |_, _, cx| lib.update(cx, |l, cx| l.set_recursive(!on, cx)))
    };

    let albums = {
        let lib = lib.clone();
        let choices = lib.read(cx).albums.clone();
        let active = lib.read(cx).active_album;
        let title = choices
            .iter()
            .find(|(id, _)| Some(*id) == active)
            .map_or_else(|| t("album.title"), |(_, name)| name.clone());
        crate::ui::labeled_button("albums", crate::ui::Glyph::Folder, title)
            .xsmall()
            .ghost()
            .dropdown_menu(move |menu, window, cx| {
                let create_lib = lib.clone();
                let menu = menu
                    .item(
                        PopupMenuItem::new(t("album.create")).on_click(move |_, _, cx| {
                            create_lib.update(cx, |_, cx| {
                                cx.emit(crate::library::LibraryEvent::CreateAlbum)
                            });
                        }),
                    )
                    .separator();
                choices.iter().fold(menu, |menu, (id, name)| {
                    let id = *id;
                    let open = lib.clone();
                    let add = lib.clone();
                    let remove = lib.clone();
                    let delete = lib.clone();
                    menu.submenu(name.clone(), window, cx, move |menu, _, _| {
                        menu.item(PopupMenuItem::new(t("album.open")).on_click({
                            let lib = open.clone();
                            move |_, _, cx| lib.update(cx, |l, cx| l.open_album(id, cx))
                        }))
                        .item(PopupMenuItem::new(t("album.add")).on_click({
                            let lib = add.clone();
                            move |_, _, cx| lib.update(cx, |l, cx| l.change_album(id, true, cx))
                        }))
                        .item(PopupMenuItem::new(t("album.remove")).on_click({
                            let lib = remove.clone();
                            move |_, _, cx| lib.update(cx, |l, cx| l.change_album(id, false, cx))
                        }))
                        .separator()
                        .item(
                            PopupMenuItem::new(t("album.delete")).on_click({
                                let lib = delete.clone();
                                move |_, _, cx| lib.update(cx, |l, cx| l.delete_album(id, cx))
                            }),
                        )
                    })
                })
            })
    };

    let stacks = {
        let lib = lib.clone();
        let collapsed = lib.read(cx).collapse_stacks;
        crate::ui::icon_button(
            "collapse-stacks",
            crate::ui::Glyph::Grid,
            t("action.ToggleStacks"),
        )
        .xsmall()
        .ghost()
        .selected(collapsed)
        .on_click(move |_, _, cx| lib.update(cx, |l, cx| l.toggle_stacks(cx)))
    };
    let duplicates = {
        let lib = lib.clone();
        let active = lib.read(cx).show_duplicates || lib.read(cx).scanning_duplicates;
        crate::ui::icon_button(
            "find-duplicates",
            crate::ui::Glyph::Copy,
            t("library.duplicates.hint"),
        )
        .xsmall()
        .ghost()
        .selected(active)
        .on_click(move |_, _, cx| lib.update(cx, |l, cx| l.toggle_duplicates(cx)))
    };

    let saved = lib.read(cx).saved_searches.clone();
    let save = {
        let lib = lib.clone();
        crate::ui::icon_button(
            "search-save",
            crate::ui::Glyph::Save,
            t("library.search.save"),
        )
        .xsmall()
        .ghost()
        .on_click(move |_, _, cx| lib.update(cx, |l, cx| l.save_search(cx)))
    };
    let load = {
        let lib = lib.clone();
        let saved = saved.clone();
        crate::ui::icon_button(
            "search-load",
            crate::ui::Glyph::Filter,
            t("library.search.saved"),
        )
        .xsmall()
        .ghost()
        .dropdown_menu(move |menu, _, _| {
            saved.iter().fold(menu, |menu, search| {
                let lib = lib.clone();
                let search = search.clone();
                menu.item(
                    PopupMenuItem::new(search.name.clone()).on_click(move |_, window, cx| {
                        lib.update(cx, |l, cx| {
                            l.apply_search(search.filter.clone(), search.sort, window, cx)
                        });
                    }),
                )
            })
        })
    };
    let remove = {
        let lib = lib.clone();
        crate::ui::icon_button(
            "search-delete",
            crate::ui::Glyph::Delete,
            t("library.search.delete"),
        )
        .xsmall()
        .ghost()
        .dropdown_menu(move |menu, _, _| {
            saved.iter().enumerate().fold(menu, |menu, (i, search)| {
                let lib = lib.clone();
                menu.item(
                    PopupMenuItem::new(search.name.clone()).on_click(move |_, _, cx| {
                        lib.update(cx, |l, cx| l.remove_search(i, cx));
                    }),
                )
            })
        })
    };

    h_flex()
        .w_full()
        .flex_wrap()
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
        .child(
            div()
                .w(px(200.0))
                .child(Input::new(&lib.read(cx).search_input).small()),
        )
        .child(albums)
        .child(duplicates)
        .child(stacks)
        .child(save)
        .child(load)
        .child(remove)
        .child(rating)
        .child(label)
        .child(camera)
        .child(lens)
        .child(date)
        .child(keyword)
        .when(active, |d| d.child(clear))
        .child(div().flex_1())
        .child(subfolders)
        .child(sort_button)
}
