use super::*;
use dbide_ui_spike::value_selection::{
    self, CopySelection, InspectionSnapshot, RowSnapshot, SelectionLimits, ValueSnapshot,
};
use gpui_component::{WindowExt, notification::Notification};

type LoadedResult = (Rc<ResultPage>, Option<(usize, usize)>, String);

pub(super) struct RangeCopyDraft {
    page: Rc<ResultPage>,
    source: String,
    inputs: [Entity<InputState>; 4],
    error: Option<String>,
}
impl Workbench {
    pub(super) fn active_loaded_result(&self, cx: &App) -> Option<LoadedResult> {
        let (grid, source) = if let Some(editor) = self.active_table_entity() {
            let editor = editor.read(cx);
            (
                editor.result_grid(),
                format!(
                    "{} · {}",
                    editor.label(),
                    dbide_ui_spike::workbench_model::profile_target(&editor.token().profile)
                ),
            )
        } else {
            if self.tools.is_some() {
                return None;
            }
            (
                self.grid.clone(),
                self.result_owner
                    .as_ref()
                    .map(|o| o.label())
                    .unwrap_or_else(|| self.file_name.clone()),
            )
        };
        let state = grid.read(cx);
        let GridSource::Postgres(page) = &state.delegate().source else {
            return None;
        };
        if page.command_only || page.columns.is_empty() {
            return None;
        }
        Some((page.clone(), state.selected_cell(), source))
    }
    pub(super) fn result_has_selection(&self, cx: &App) -> bool {
        self.active_loaded_result(cx)
            .is_some_and(|(_, s, _)| s.is_some())
    }
    pub(super) fn result_has_rows(&self, cx: &App) -> bool {
        self.active_loaded_result(cx)
            .is_some_and(|(p, _, _)| !p.rows.is_empty())
    }
    fn show_inspection(&mut self, record: bool, w: &mut Window, cx: &mut Context<Self>) {
        let Some((page, Some((row, column)), source)) = self.active_loaded_result(cx) else {
            return;
        };
        let snapshot = if record {
            RowSnapshot::capture(&page, source, row).map(InspectionSnapshot::Row)
        } else {
            ValueSnapshot::capture(&page, source, row, column).map(InspectionSnapshot::Value)
        };
        match snapshot {
            Ok(snapshot) => {
                let panel =
                    cx.new(|cx| value_inspector::ValueInspector::new(snapshot, self.locale, w, cx));
                self.inspector_subscription = Some(cx.subscribe(&panel, |s, source, _, cx| {
                    if s.value_inspector
                        .as_ref()
                        .is_some_and(|v| v.entity_id() == source.entity_id())
                    {
                        s.value_inspector = None;
                        cx.notify();
                    }
                }));
                self.preferences = None;
                self.form = None;
                self.cancel_object_reveal();
                self.value_inspector = Some(panel);
                self.stream_export_visible = false;
                self.range_copy = None;
                self.structure = None;
                self.object_finder = None;
                self.show_history = false;
            }
            Err(error) => w.push_notification(Notification::error(error.to_string()), cx),
        }
        cx.notify();
    }
    pub(super) fn inspect_value_action(
        &mut self,
        _: &InspectValue,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_inspection(false, w, cx);
    }
    pub(super) fn inspect_record_action(
        &mut self,
        _: &InspectRecord,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_inspection(true, w, cx);
    }
    fn copy_result_axis(&mut self, row_axis: bool, w: &mut Window, cx: &mut Context<Self>) {
        let Some((page, Some((row, column)), _)) = self.active_loaded_result(cx) else {
            return;
        };
        let range = if row_axis {
            CopySelection::Row(row)
        } else {
            CopySelection::Column(column)
        };
        match value_selection::copy_selection(&page, range, SelectionLimits::default()) {
            Ok(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                w.push_notification(
                    Notification::success(if self.locale == Locale::Ko {
                        "보관된 결과를 TSV로 복사했습니다. NULL은 \\N입니다."
                    } else {
                        "Copied loaded values as TSV. SQL NULL is \\N."
                    }),
                    cx,
                );
            }
            Err(error) => w.push_notification(Notification::error(error.to_string()), cx),
        }
    }
    pub(super) fn copy_result_row(
        &mut self,
        _: &CopyResultRow,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_result_axis(true, w, cx);
    }
    pub(super) fn copy_result_column(
        &mut self,
        _: &CopyResultColumn,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_result_axis(false, w, cx);
    }
    pub(super) fn open_range_copy(
        &mut self,
        _: &CopyResultRange,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((page, selection, source)) = self.active_loaded_result(cx) else {
            return;
        };
        if page.rows.is_empty() {
            return;
        }
        let (row, column) = selection.unwrap_or((0, 0));
        let values = [row + 1, row + 1, column + 1, column + 1];
        let inputs = values.map(|value| {
            cx.new(|cx| {
                let mut input = InputState::new(w, cx);
                input.set_value(value.to_string(), w, cx);
                input
            })
        });
        self.preferences = None;
        self.form = None;
        self.cancel_object_reveal();
        self.stream_export_visible = false;
        self.range_copy = Some(RangeCopyDraft {
            page,
            source,
            inputs,
            error: None,
        });
        self.value_inspector = None;
        self.object_finder = None;
        self.structure = None;
        self.show_history = false;
        cx.notify();
    }
    fn copy_range(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = &mut self.range_copy else {
            return;
        };
        let parsed: Result<Vec<usize>, _> = draft
            .inputs
            .iter()
            .map(|input| input.read(cx).value().trim().parse::<usize>())
            .collect();
        let result = match parsed {
            Ok(v) if v[0] > 0 && v[2] > 0 && v[1] >= v[0] && v[3] >= v[2] => {
                value_selection::copy_selection(
                    &draft.page,
                    CopySelection::Range {
                        rows: v[0] - 1..v[1],
                        columns: v[2] - 1..v[3],
                    },
                    SelectionLimits::default(),
                )
                .map_err(|e| e.to_string())
            }
            _ => Err(if self.locale == Locale::Ko {
                "시작·끝에 올바른 1부터의 행/열 번호를 입력하세요."
            } else {
                "Enter valid 1-based start/end row and column numbers."
            }
            .into()),
        };
        match result {
            Ok(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                draft.error = None;
                w.push_notification(
                    Notification::success(if self.locale == Locale::Ko {
                        "선택 범위를 TSV로 복사했습니다."
                    } else {
                        "Copied range as TSV."
                    }),
                    cx,
                );
            }
            Err(error) => draft.error = Some(error),
        }
        cx.notify();
    }
    pub(super) fn range_copy_panel(&self, cx: &mut Context<Self>) -> Div {
        let draft = self.range_copy.as_ref().expect("range copy panel");
        let ko = self.locale == Locale::Ko;
        let labels = if ko {
            ["시작 행", "끝 행", "시작 열", "끝 열"]
        } else {
            ["First row", "Last row", "First column", "Last column"]
        };
        div().size_full().flex_shrink_0().bg(rgb(PANEL)).border_l_1().border_color(rgb(BORDER)).p_4().flex().flex_col().gap_3()
            .child(div().flex().justify_between().child(if ko{"범위 복사"}else{"Copy range"}).child(self.button("close-range-copy","Close").on_click(cx.listener(|s,_,_,cx|{s.range_copy=None;cx.notify();}))))
            .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(draft.source.clone()))
            .child(format!("{} × {} · {}",draft.page.rows.len(),draft.page.columns.len(),if ko{"보관된 결과 · 끝 번호 포함"}else{"Loaded snapshot · inclusive bounds"}))
            .children(labels.into_iter().enumerate().map(|(i,label)|div().flex().items_center().gap_2().child(div().w(px(95.)).child(label)).child(Input::new(&draft.inputs[i]).small().aria_label(label))))
            .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(if ko{"최대 10,000셀 / 1 MiB · SQL NULL은 \\N, 빈 문자열은 빈 필드. 탭·줄바꿈은 TSV 인용으로 보존됩니다."}else{"Up to 10,000 cells / 1 MiB. SQL NULL is \\N; empty text is an empty field. Tabs/newlines use TSV quoting."}))
            .when_some(draft.error.as_ref(),|el,error|el.child(div().text_color(rgb(0xe8ad93)).child(error.clone())))
            .child(self.button("copy-loaded-range",if ko {"복사"} else {"Copy"}).on_click(cx.listener(|s,_,w,cx|s.copy_range(w,cx))))
    }
}
