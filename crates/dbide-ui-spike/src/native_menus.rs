use super::*;
pub(super) fn menus(normal: bool, can_run: bool, cancellable: bool) -> Vec<Menu> {
    vec![
        Menu::new("Rust Database IDE").items([MenuItem::action("Quit Rust Database IDE", Quit)]),
        Menu::new("File").items([
            MenuItem::action("New SQL", NewSqlFile).disabled(!normal),
            MenuItem::action("Open SQL…", OpenSqlFile).disabled(!normal),
            MenuItem::separator(),
            MenuItem::action("Save SQL", SaveSqlFile).disabled(!normal),
            MenuItem::action("Save SQL As…", SaveSqlAs).disabled(!normal),
            MenuItem::action("Close Tab / Panel", CloseSqlTab),
        ]),
        Menu::new("Query").items([
            MenuItem::action("Run Script", RunQuery).disabled(!can_run),
            MenuItem::action("Run Current Statement", RunCurrent).disabled(!can_run),
            MenuItem::action("Cancel Query", CancelQuery).disabled(!cancellable),
            MenuItem::separator(),
            MenuItem::action("Format SQL", FormatSql).disabled(!normal),
            MenuItem::action("Complete SQL", CompleteSql).disabled(!normal),
        ]),
    ]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn native_query_menu_never_admits_run_or_cancel_when_unavailable() {
        let menus = menus(true, false, false);
        assert_eq!(menus.len(), 3);
        for item in menus[2].items.iter().take(3) {
            assert!(matches!(item, MenuItem::Action { disabled: true, .. }));
        }
        assert!(matches!(
            menus[0].items[0],
            MenuItem::Action {
                disabled: false,
                ..
            }
        ));
        let active = super::menus(true, true, true);
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
