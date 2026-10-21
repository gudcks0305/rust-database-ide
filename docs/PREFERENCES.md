# Application language preferences

`dbide-settings` is a lightweight, GPUI-free settings store usable by both the
native application and CLI. `dbide-preferences-ui` provides the actual native
Korean/English choice component. The host owns mounting, background persistence,
application locale switching and redraw; neither helper edits recovery settings,
profiles, history or existing UI files.

## Shared settings API

```rust,no_run
use dbide_settings::{AppSettings, Locale, SettingsStore};
let store = SettingsStore::for_profile_path("/explicit/workspace/profiles.json");
assert_eq!(store.path(), std::path::Path::new("/explicit/workspace/app-settings.json"));
let settings: AppSettings = store.load()?; // missing file => Korean; creates nothing
store.set_locale(Locale::En)?;
store.save(&AppSettings { locale: Locale::Ko })?;
# Ok::<(), dbide_settings::StoreError>(())
```

`SettingsStore::new(path)` accepts any explicitly selected settings path;
`for_profile_path(profile_path)` is a pure shared path convention selecting
`profile_path.parent()/app-settings.json`. It never opens the profile file,
looks up HOME or discovers user directories. Relative bare profile filenames use
`./app-settings.json`. CLI language get/set can work even if the profile JSON is
malformed because profile data is unrelated to application language preferences.
The host should do actual settings I/O off the UI thread.

`AppSettings { locale: Locale }` defaults to Korean and uses the same stable
`Locale::{Ko, En}` JSON `"ko"`/`"en"` as `dbide-i18n`. Stored documents are:

```json
{"schema_version":1,"locale":"ko"}
```

Unknown fields, including nested values, are preserved semantically on every
save. JSON whitespace/key ordering may change. Only `locale` is mutated. Missing
known fields, duplicate schema/locale fields, invalid locale values, malformed or
non-UTF-8 data, unsupported schema versions and files larger than 64 KiB are
rejected. Mutation revalidates the latest file under an advisory writer lock;
loading a valid snapshot earlier cannot authorize overwriting a newer corrupt
or future-version file. Failed validation leaves original bytes unchanged.

Each writer opens the same sibling `<settings-filename>.lock`, obtains the OS
file lock, re-reads the latest document, updates locale, then prepares a unique
same-directory temporary file. The file is flushed/synced and atomically renamed
over the active path; the containing directory is synced on Unix. Newly created
settings/temp/lock files are 0600 and new directories are 0700 on Unix. Existing
parent-directory permissions are preserved. A lock file remains as a reusable
coordination point; OS lock ownership ends when the operation finishes.

Settings and lock endpoints must be regular files. Symlinks and special files
are rejected before opening; Unix opens also use `O_NONBLOCK | O_NOFOLLOW`, then
validate the opened descriptor, so FIFO opens cannot block and a final-path
replacement cannot cause a symlink read. Caller-supplied parent directories must
be trusted; this is not a sandbox against a process concurrently replacing
ancestor directories. Advisory locking coordinates cooperating UI/CLI writers.
No setting, recovery snapshot or user file is deleted by this helper.

## Native panel contract

```rust,ignore
use dbide_preferences_ui::{PreferencesPanel, PreferencesEvent};
let panel = cx.new(|cx| PreferencesPanel::new(current_locale, window, cx));
// Subscribe to PreferencesEvent::LocaleChosen(locale).
// Save with the shared SettingsStore in a background task.
// Only after accepting/persisting the choice, update application locale and:
// panel.update(cx, |p, cx| p.set_locale(locale, cx));
```

The panel displays a real 한국어 / English choice with the applied language
highlighted, catalog-translated title/guidance and a concise bilingual note that
SQL, identifiers and original server diagnostics remain unchanged. Choices emit
`PreferencesEvent::LocaleChosen(Locale)` only on user click. Construction and
rendering perform no I/O. The panel itself does not claim a selection was saved
or apply a process-global locale; on failure the host can keep the old locale
and show the actual persistence error. `set_locale` updates the applied selection
when the host confirms it. No placeholder advanced-preference controls are shown.

## Verification

```sh
cargo test -p dbide-settings -p dbide-i18n
cargo check -p dbide-preferences-ui
cargo clippy -p dbide-settings -p dbide-i18n -p dbide-preferences-ui --all-targets -- -D warnings
```

Settings tests use temporary fixtures only: no-side-effect Korean default,
save/reload/permissions, explicit sibling-path convention, nested Unicode unknown
field preservation, stale snapshot plus newer fields, future/corrupt/ambiguous
file preservation, UTF-8 byte bounds, symlink/FIFO/lock endpoint rejection, and
concurrent cooperating writers. The native panel is compile checked; actual
menu entry, persistence error presentation, language redraw and native GUI QA
are host integration responsibilities. Core catalog completeness and placeholder
parity remain covered by `dbide-i18n` tests.
