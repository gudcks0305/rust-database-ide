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
            MenuItem::action(tr(locale, Message::NewConsole), NewSqlFile).disabled(!normal),
            MenuItem::action(tr(locale, Message::Open), OpenSqlFile).disabled(!normal),
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
        ]),
    ]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn native_query_menu_never_admits_run_or_cancel_when_unavailable() {
        let menus = menus(Locale::En, true, false, false);
        assert_eq!(menus.len(), 3);
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
