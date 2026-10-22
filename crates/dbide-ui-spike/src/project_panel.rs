//! Explicit folder workspace browser. Filesystem work stays on the disk executor.
use dbide_i18n::Locale;
use dbide_project::{DirectoryListing, EntryKind, Manifest, Project, ProjectError, SqlFile};
use gpui::{prelude::*, *};
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    tooltip::Tooltip,
};
use std::path::{Path, PathBuf};

pub enum ProjectPanelEvent {
    Open {
        file: SqlFile,
        manifest: Box<Manifest>,
    },
    BindCurrent {
        expected_revision: u64,
    },
    ChooseProject,
    CloseProject,
}

#[derive(Clone, Copy)]
enum CreateKind {
    Sql,
    Folder,
}

pub struct ProjectPanel {
    project: Project,
    manifest: Manifest,
    locale: Locale,
    directory: PathBuf,
    listing: Option<DirectoryListing>,
    generation: u64,
    busy: bool,
    error: Option<String>,
    creating: Option<CreateKind>,
    name: Entity<InputState>,
    scroll: ScrollHandle,
    _subscription: Subscription,
}
impl EventEmitter<ProjectPanelEvent> for ProjectPanel {}

fn text(locale: Locale, ko: &'static str, en: &'static str) -> &'static str {
    if locale == Locale::Ko { ko } else { en }
}

impl ProjectPanel {
    pub fn new(
        project: Project,
        manifest: Manifest,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx));
        let subscription = cx.subscribe(&name, |s, _, event, cx| match event {
            InputEvent::PressEnter { .. } => s.create(cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        let mut panel = Self {
            project,
            manifest,
            locale,
            directory: PathBuf::new(),
            listing: None,
            generation: 0,
            busy: false,
            error: None,
            creating: None,
            name,
            scroll: ScrollHandle::new(),
            _subscription: subscription,
        };
        panel.refresh(cx);
        panel
    }

    pub fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.locale = locale;
        cx.notify();
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn current_directory(&self) -> &Path {
        &self.directory
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.load_directory(self.directory.clone(), cx);
    }

    fn load_directory(&mut self, directory: PathBuf, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let project = self.project.clone();
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    Ok::<_, ProjectError>((project.manifest()?, project.list(&directory)?))
                })
                .await;
            let _ = view.update(cx, |s, cx| {
                if s.generation != generation {
                    return;
                }
                s.busy = false;
                match result {
                    Ok((manifest, listing)) => {
                        s.manifest = manifest;
                        s.directory = listing.relative_path.clone();
                        s.listing = Some(listing);
                        s.scroll.set_offset(point(px(0.), px(0.)));
                    }
                    Err(error) => s.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open(&mut self, relative: PathBuf, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let project = self.project.clone();
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    Ok::<_, ProjectError>((project.read_sql(&relative)?, project.manifest()?))
                })
                .await;
            let _ = view.update(cx, |s, cx| {
                if s.generation != generation {
                    return;
                }
                s.busy = false;
                match result {
                    Ok((file, manifest)) => {
                        s.manifest = manifest.clone();
                        cx.emit(ProjectPanelEvent::Open {
                            file,
                            manifest: Box::new(manifest),
                        });
                    }
                    Err(error) => s.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn begin_create(&mut self, kind: CreateKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.creating = Some(kind);
        self.error = None;
        self.name.update(cx, |input, cx| {
            input.set_value(
                if matches!(kind, CreateKind::Sql) {
                    "query.sql"
                } else {
                    ""
                },
                window,
                cx,
            );
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.creating else { return };
        if self.busy {
            return;
        }
        let name = self.name.read(cx).value().to_string();
        // The UI creates a single child; the core additionally validates every
        // path component and rejects symlinks and existing destinations.
        if name.is_empty() || name.contains(['/', '\\']) || name != name.trim() {
            self.error = Some(
                text(
                    self.locale,
                    "현재 폴더에 만들 이름 하나를 입력하세요.",
                    "Enter one name in the current folder.",
                )
                .into(),
            );
            cx.notify();
            return;
        }
        let relative = self.directory.join(name);
        let directory = self.directory.clone();
        let project = self.project.clone();
        self.busy = true;
        self.error = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let file = match kind {
                        CreateKind::Sql => Some(project.write_sql(&relative, "", None)?),
                        CreateKind::Folder => {
                            project.mkdir(&relative)?;
                            None
                        }
                    };
                    let manifest = project.manifest()?;
                    // A listing failure must not disguise a successful creation.
                    let listing = project.list(&directory);
                    Ok::<_, ProjectError>((file, manifest, listing))
                })
                .await;
            let _ = view.update(cx, |s, cx| {
                if s.generation != generation {
                    return;
                }
                s.busy = false;
                match result {
                    Ok((file, manifest, listing)) => {
                        s.creating = None;
                        s.manifest = manifest.clone();
                        match listing {
                            Ok(listing) => s.listing = Some(listing),
                            Err(error) => s.error = Some(error.to_string()),
                        }
                        if let Some(file) = file {
                            cx.emit(ProjectPanelEvent::Open {
                                file,
                                manifest: Box::new(manifest),
                            });
                        }
                    }
                    Err(error) => s.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for ProjectPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.locale;
        let busy = self.busy;
        let weak = cx.weak_entity();
        let new = Button::new("project-new")
            .small()
            .label(text(locale, "새로 만들기 ▾", "New ▾"))
            .disabled(busy)
            .dropdown_menu(move |menu, _, _| {
                let sql = weak.clone();
                let folder = weak.clone();
                menu.item(
                    PopupMenuItem::new(text(locale, "SQL 파일", "SQL file")).on_click(
                        move |_, w, cx| {
                            let _ = sql.update(cx, |s, cx| s.begin_create(CreateKind::Sql, w, cx));
                        },
                    ),
                )
                .item(PopupMenuItem::new(text(locale, "폴더", "Folder")).on_click(
                    move |_, w, cx| {
                        let _ =
                            folder.update(cx, |s, cx| s.begin_create(CreateKind::Folder, w, cx));
                    },
                ))
            });
        let rows = self
            .listing
            .as_ref()
            .map(|listing| {
                listing
                    .entries
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| {
                        let relative = entry.relative_path.clone();
                        let directory = entry.kind == EntryKind::Directory;
                        Button::new(("project-entry", index))
                            .small()
                            .ghost()
                            .justify_start()
                            .w_full()
                            .label(format!(
                                "{} {}",
                                if directory { "▸" } else { "SQL" },
                                entry.name
                            ))
                            .accessibility_label(format!(
                                "{} {}",
                                text(
                                    locale,
                                    if directory {
                                        "폴더 열기"
                                    } else {
                                        "SQL 파일 열기"
                                    },
                                    if directory {
                                        "Open folder"
                                    } else {
                                        "Open SQL file"
                                    }
                                ),
                                entry.relative_path.display()
                            ))
                            .disabled(busy)
                            .on_click(cx.listener(move |s, _, _, cx| {
                                if directory {
                                    s.load_directory(relative.clone(), cx);
                                } else {
                                    s.open(relative.clone(), cx);
                                }
                            }))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let breadcrumb = if self.directory.as_os_str().is_empty() {
            "/".to_string()
        } else {
            format!("/{}", self.directory.display())
        };
        div()
            .id("project-panel")
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .gap_2()
            .px_2()
            .py_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.manifest.name.clone()),
                    )
                    .child(
                        Button::new("project-refresh")
                            .small()
                            .ghost()
                            .label(text(locale, "새로고침", "Refresh"))
                            .disabled(busy)
                            .on_click(cx.listener(|s, _, _, cx| s.refresh(cx))),
                    ),
            )
            .child(
                div()
                    .id("project-root-path")
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(px(11.))
                    .text_color(rgb(0x9aa9bd))
                    .child(self.project.root().display().to_string())
                    .tooltip({
                        let path = self.project.root().display().to_string();
                        move |w, cx| Tooltip::new(path.clone()).build(w, cx)
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("project-choose")
                            .small()
                            .label(text(locale, "프로젝트 열기", "Open project"))
                            .disabled(busy)
                            .on_click(
                                cx.listener(|_, _, _, cx| {
                                    cx.emit(ProjectPanelEvent::ChooseProject)
                                }),
                            ),
                    )
                    .child(
                        Button::new("project-close")
                            .small()
                            .label(text(locale, "닫기", "Close"))
                            .disabled(busy)
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(ProjectPanelEvent::CloseProject)),
                            ),
                    ),
            )
            .child(
                div()
                    .border_b_1()
                    .border_color(rgb(0x2a3644))
                    .pb_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(0x9aa9bd))
                            .child(text(locale, "기본 실행 대상", "Default execution target")),
                    )
                    .child(
                        div().text_size(px(12.)).child(
                            self.manifest
                                .default_target
                                .as_ref()
                                .map(|target| {
                                    format!("{} / {}", target.profile_id, target.database)
                                })
                                .unwrap_or_else(|| text(locale, "지정되지 않음", "Not set").into()),
                        ),
                    )
                    .child(
                        Button::new("project-bind-target")
                            .small()
                            .label(text(
                                locale,
                                "현재 콘솔 대상 저장",
                                "Use current console target",
                            ))
                            .disabled(busy)
                            .on_click(cx.listener(|s, _, _, cx| {
                                cx.emit(ProjectPanelEvent::BindCurrent {
                                    expected_revision: s.manifest.revision,
                                })
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("project-up")
                            .small()
                            .label(text(locale, "상위", "Up"))
                            .disabled(busy || self.directory.as_os_str().is_empty())
                            .on_click(cx.listener(|s, _, _, cx| {
                                let parent =
                                    s.directory.parent().unwrap_or(Path::new("")).to_path_buf();
                                s.load_directory(parent, cx);
                            })),
                    )
                    .child(div().flex_1().min_w_0().text_ellipsis().child(breadcrumb))
                    .child(new),
            )
            .when_some(self.creating, |panel, kind| {
                panel.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_size(px(12.)).child(text(
                            locale,
                            if matches!(kind, CreateKind::Sql) {
                                "새 SQL 파일 (.sql)"
                            } else {
                                "새 폴더"
                            },
                            if matches!(kind, CreateKind::Sql) {
                                "New SQL file (.sql)"
                            } else {
                                "New folder"
                            },
                        )))
                        .child(
                            Input::new(&self.name)
                                .small()
                                .disabled(busy)
                                .aria_label(text(
                                    locale,
                                    "새 파일 또는 폴더 이름",
                                    "New file or folder name",
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    Button::new("project-create")
                                        .small()
                                        .label(text(locale, "만들기", "Create"))
                                        .disabled(busy)
                                        .on_click(cx.listener(|s, _, _, cx| s.create(cx))),
                                )
                                .child(
                                    Button::new("project-create-cancel")
                                        .small()
                                        .label(text(locale, "취소", "Cancel"))
                                        .disabled(busy)
                                        .on_click(cx.listener(|s, _, _, cx| {
                                            s.creating = None;
                                            s.error = None;
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
            .when_some(self.error.clone(), |panel, error| {
                panel.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(0xf0a0a0))
                        .child(error),
                )
            })
            .when(busy, |panel| {
                panel.child(
                    div()
                        .text_size(px(11.))
                        .child(text(locale, "처리 중…", "Working…")),
                )
            })
            .child(
                div()
                    .id("project-entries")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(rows)
                    .when(
                        self.listing
                            .as_ref()
                            .is_some_and(|listing| listing.entries.is_empty()),
                        |list| {
                            list.child(
                                div()
                                    .py_2()
                                    .text_size(px(12.))
                                    .text_color(rgb(0x9aa9bd))
                                    .child(text(
                                        locale,
                                        "폴더 또는 SQL 파일 없음",
                                        "No folders or SQL files",
                                    )),
                            )
                        },
                    ),
            )
            .when(
                self.listing
                    .as_ref()
                    .is_some_and(|listing| listing.truncated),
                |panel| {
                    panel.child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(0xf0c080))
                            .child(text(
                                locale,
                                "일부 항목만 표시됨 · 최대 512개, 4,096개 스캔",
                                "Partial listing · up to 512 entries / 4,096 scanned",
                            )),
                    )
                },
            )
    }
}
