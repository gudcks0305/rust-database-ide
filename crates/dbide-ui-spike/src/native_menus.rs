use super::*;
pub(super) fn menus(locale: Locale, normal: bool, can_run: bool, cancellable: bool) -> Vec<Menu> {
    vec![
        Menu::new("Rust Database IDE").items([
            MenuItem::action(tr(locale, Message::Preferences), OpenPreferences),
            MenuItem::separator(),
            MenuItem::action(
                if locale == Locale::Ko {
                    "데이터베이스 IDE 종료"
                } else {
                    "Quit Rust Database IDE"
                },
                Quit,
            ),
        ]),
        Menu::new(tr(locale, Message::MenuFile)).items([
            MenuItem::action(tr(locale, Message::NewConsole), NewSqlFile),
            MenuItem::action(tr(locale, Message::Open), OpenSqlFile),
            MenuItem::separator(),
            MenuItem::action(tr(locale, Message::Save), SaveSqlFile).disabled(!normal),
            MenuItem::action(
                if locale == Locale::Ko {
                    "다른 이름으로 저장…"
                } else {
                    "Save SQL As…"
                },
                SaveSqlAs,
            )
            .disabled(!normal),
            MenuItem::action(tr(locale, Message::Close), CloseSqlTab),
            MenuItem::separator(),
            MenuItem::action(
                if locale == Locale::Ko {
                    "새 워크스페이스…"
                } else {
                    "New workspace…"
                },
                NewProject,
            ),
            MenuItem::action(
                if locale == Locale::Ko {
                    "워크스페이스 열기…"
                } else {
                    "Open workspace…"
                },
                OpenProject,
            ),
            MenuItem::separator(),
        ]),
        Menu::new(tr(locale, Message::MenuQuery)).items([
            MenuItem::action(tr(locale, Message::Run), RunQuery).disabled(!can_run),
            MenuItem::action(
                if locale == Locale::Ko {
                    "현재 구문 실행"
                } else {
                    "Run current statement"
                },
                RunCurrent,
            )
            .disabled(!can_run),
            MenuItem::action(tr(locale, Message::Cancel), CancelQuery).disabled(!cancellable),
            MenuItem::action(
                if locale == Locale::Ko {
                    "선택 영역 실행"
                } else {
                    "Run selection"
                },
                RunSelection,
            )
            .disabled(!normal || !can_run),
            MenuItem::separator(),
            MenuItem::action(
                if locale == Locale::Ko {
                    "SQL 서식"
                } else {
                    "Format SQL"
                },
                FormatSql,
            )
            .disabled(!normal),
            MenuItem::action(
                if locale == Locale::Ko {
                    "SQL 자동 완성"
                } else {
                    "Complete SQL"
                },
                CompleteSql,
            )
            .disabled(!normal),
            MenuItem::action(
                if locale == Locale::Ko {
                    "현재 콘솔 완성 메타데이터 새로고침"
                } else {
                    "Refresh current console completion metadata"
                },
                RefreshCompletionMetadata,
            )
            .disabled(!normal),
        ]),
        Menu::new(if locale == Locale::Ko {
            "이동"
        } else {
            "Navigate"
        })
        .items([
            MenuItem::action(
                if locale == Locale::Ko {
                    "다음 탭"
                } else {
                    "Next tab"
                },
                NextWorkspaceTab,
            ),
            MenuItem::action(
                if locale == Locale::Ko {
                    "이전 탭"
                } else {
                    "Previous tab"
                },
                PreviousWorkspaceTab,
            ),
            MenuItem::separator(),
            MenuItem::action(
                if locale == Locale::Ko {
                    "객체 찾기…"
                } else {
                    "Find object…"
                },
                FindDatabaseObject,
            ),
            MenuItem::action(
                if locale == Locale::Ko {
                    "쿼리 기록 검색"
                } else {
                    "Search query history"
                },
                OpenQueryHistory,
            ),
            MenuItem::action(
                if locale == Locale::Ko {
                    "탐색기로 이동"
                } else {
                    "Focus database explorer"
                },
                FocusCatalog,
            ),
            MenuItem::action(
                if locale == Locale::Ko {
                    "결과로 이동"
                } else {
                    "Focus results"
                },
                FocusResults,
            ),
        ]),
        Menu::new(if locale == Locale::Ko {
            "보기"
        } else {
            "View"
        })
        .items([
            MenuItem::action(
                if locale == Locale::Ko {
                    "탐색기 표시 전환"
                } else {
                    "Toggle database explorer"
                },
                ToggleCatalog,
            ),
            MenuItem::action(
                if locale == Locale::Ko {
                    "결과 표시 전환"
                } else {
                    "Toggle results"
                },
                ToggleResults,
            ),
        ]),
    ]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn native_query_menu_never_admits_run_or_cancel_when_unavailable() {
        let table_view = menus(Locale::En, false, false, false);
        for item in table_view[1].items.iter().take(2) {
            assert!(matches!(
                item,
                MenuItem::Action {
                    disabled: false,
                    ..
                }
            ));
        }
        for index in [3, 4] {
            assert!(matches!(
                table_view[1].items[index],
                MenuItem::Action { disabled: true, .. }
            ));
        }
        let menus = menus(Locale::En, true, false, false);
        assert_eq!(menus.len(), 5);
        for item in menus[2].items.iter().take(3) {
            assert!(matches!(item, MenuItem::Action { disabled: true, .. }));
        }
        assert!(matches!(
            menus[0].items[2],
            MenuItem::Action {
                disabled: false,
                ..
            }
        ));
        let active = super::menus(Locale::En, true, true, true);
        for item in active[2].items.iter().take(3) {
            assert!(matches!(
                item,
                MenuItem::Action {
                    disabled: false,
                    ..
                }
            ));
        }
    }
}
