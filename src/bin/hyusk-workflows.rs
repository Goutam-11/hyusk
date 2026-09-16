//! THESIS: Make an automation read like a clear sequence of desktop actions,
//! refusing the cramped settings-form editor used by the Shell extension.
//! OWN-WORLD: Native Adwaita surfaces, blue-violet Hyusk accents, a searchable
//! library rail, and full-width action rows with platform-standard controls.
//! STORY: Find or create a workflow, understand its voice triggers and ordered
//! actions, save it safely, then run it through the Hyusk service.
//! FIRST VIEWPORT: Workflow library and search on the left; focused name,
//! triggers, action sequence, save, and run controls on the right.
//! FORM: Native GNOME utility following the requested Shortcuts-style canon.
//! FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, and DESIGN.md
//!
//! Native GNOME workflow editor for Hyusk.
//!
//! The editor deliberately speaks the version-one `commands.json` format. It
//! owns editing and persistence, while the long-running Hyusk service remains
//! the only process that executes actions. Run requests are sent through the
//! same small runtime control file used by the GNOME extension.

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    cell::RefCell,
    collections::HashSet,
    error::Error,
    fs,
    path::PathBuf,
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

type AppResult<T> = Result<T, Box<dyn Error>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Workflow {
    name: String,
    #[serde(default)]
    phrases: Vec<String>,
    #[serde(default)]
    steps: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct Config {
    // Keep apps, sites, and future top-level keys intact when the editor saves.
    root: Map<String, Value>,
    workflows: Vec<Workflow>,
}

struct AppState {
    config: Config,
    selected: Option<usize>,
}

#[derive(Clone, Debug)]
struct DesktopApp {
    name: String,
    command: Vec<String>,
}

struct Ui {
    window: adw::ApplicationWindow,
    list_box: gtk::ListBox,
    search: gtk::SearchEntry,
    editor_title: gtk::Label,
    name_entry: gtk::Entry,
    phrases_view: gtk::TextView,
    steps_box: gtk::Box,
    status: gtk::Label,
    save_button: gtk::Button,
    delete_button: gtk::Button,
    run_button: gtk::Button,
}

fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    base.join("hyusk").join("commands.json")
}

fn control_path() -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime).join("hyusk-control.json")
}

fn normalize_workflow(mut workflow: Workflow) -> Option<Workflow> {
    workflow.name = workflow.name.trim().to_string();
    workflow.phrases = split_items(&workflow.phrases.join("\n"));
    workflow.steps = workflow
        .steps
        .into_iter()
        .map(|step| step.trim().to_string())
        .filter(|step| !step.is_empty())
        .collect();
    (!workflow.name.is_empty()).then_some(workflow)
}

fn load_config() -> Config {
    let root = fs::read_to_string(config_path())
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();

    let workflows = root
        .get("workflows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| serde_json::from_value::<Workflow>(value.clone()).ok())
        .filter_map(normalize_workflow)
        .collect();

    Config { root, workflows }
}

fn save_config(config: &Config) -> AppResult<()> {
    let path = config_path();
    let parent = path
        .parent()
        .ok_or("commands.json has no parent directory")?;
    fs::create_dir_all(parent)?;

    let mut root = config.root.clone();
    root.insert(
        "workflows".to_string(),
        serde_json::to_value(&config.workflows)?,
    );
    let bytes = serde_json::to_vec_pretty(&Value::Object(root))?;

    // The service can read this file at any time. Rename a complete temporary
    // file into place so it never observes half-written JSON.
    let temporary = parent.join(format!(".commands.json.{}.tmp", std::process::id()));
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn write_control(action: &str, text: &str) -> AppResult<()> {
    let path = control_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let revision = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let payload = serde_json::json!({
        "revision": revision,
        "action": action,
        "text": text,
    });
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temporary, serde_json::to_vec(&payload)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn write_schedule_control(action: &str, payload: Value) -> AppResult<()> {
    write_control(action, &serde_json::to_string(&payload)?)
}

fn split_items(text: &str) -> Vec<String> {
    let mut items = Vec::new();
    for item in text.lines().flat_map(|line| line.split(',')) {
        let item = item.trim();
        if !item.is_empty() && !items.iter().any(|known| known == item) {
            items.push(item.to_string());
        }
    }
    items
}

fn read_phrases(ui: &Ui) -> Vec<String> {
    let buffer = ui.phrases_view.buffer();
    let start = buffer.start_iter();
    let end = buffer.end_iter();
    split_items(&buffer.text(&start, &end, false))
}

fn read_steps(ui: &Ui) -> Vec<String> {
    let mut result = Vec::new();
    let mut child = ui.steps_box.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        let Ok(row) = widget.downcast::<gtk::Box>() else {
            continue;
        };
        let Some(entry_widget) = row.first_child() else {
            continue;
        };
        let Ok(entry) = entry_widget.downcast::<gtk::Entry>() else {
            continue;
        };
        let value = entry.text().trim().to_string();
        if !value.is_empty() {
            result.push(value);
        }
    }
    result
}

fn set_status(ui: &Ui, message: &str, error: bool) {
    ui.status.set_text(message);
    ui.status.remove_css_class("error");
    ui.status.remove_css_class("success");
    ui.status
        .add_css_class(if error { "error" } else { "success" });
}

fn desktop_app_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/usr/share/applications")];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/share/applications"));
        dirs.push(home.join(".local/share/flatpak/exports/share/applications"));
    }
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    dirs
}

fn desktop_exec_command(raw: &str) -> Vec<String> {
    raw.split_whitespace()
        .filter(|token| !token.starts_with('%'))
        .map(|token| token.trim_matches('"').trim_matches('\'').to_string())
        .filter(|token| !token.is_empty())
        .collect()
}

fn installed_apps() -> Vec<DesktopApp> {
    let mut apps = Vec::new();
    let mut seen = HashSet::new();

    for directory in desktop_app_dirs() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("desktop") {
                continue;
            }
            let Ok(contents) = fs::read_to_string(path) else {
                continue;
            };
            let mut name = None;
            let mut exec = None;
            let mut in_desktop_entry = false;
            for line in contents.lines() {
                if line.starts_with('[') {
                    in_desktop_entry = line == "[Desktop Entry]";
                    continue;
                }
                if !in_desktop_entry {
                    continue;
                }
                if let Some(value) = line.strip_prefix("Name=") {
                    name = Some(value.trim().to_string());
                } else if let Some(value) = line.strip_prefix("Exec=") {
                    exec = Some(value.trim().to_string());
                }
            }
            let Some(name) = name.filter(|value| !value.is_empty()) else {
                continue;
            };
            let Some(command) = exec.map(|value| desktop_exec_command(&value)) else {
                continue;
            };
            if command.is_empty() {
                continue;
            }
            let key = name.to_lowercase();
            if seen.insert(key) {
                apps.push(DesktopApp { name, command });
            }
        }
    }

    apps.sort_by_key(|app| app.name.to_lowercase());
    apps
}

fn register_app_alias(state: &Rc<RefCell<AppState>>, app: &DesktopApp) {
    let mut current = state.borrow_mut();
    let apps = current
        .config
        .root
        .entry("apps".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !apps.is_object() {
        *apps = Value::Object(Map::new());
    }
    if let Some(object) = apps.as_object_mut() {
        object.insert(app.name.to_lowercase(), serde_json::json!(app.command));
    }
}

fn append_app_step(ui: &Rc<Ui>, state: &Rc<RefCell<AppState>>, app: &DesktopApp) {
    register_app_alias(state, app);
    add_step_with_value(ui, state, &format!("open {}", app.name));
    set_status(
        ui,
        &format!(
            "Added {}. Save the workflow to keep this app alias.",
            app.name
        ),
        false,
    );
}

fn rebuild_app_rows(
    list: &gtk::ListBox,
    apps: &Rc<Vec<DesktopApp>>,
    query: &str,
    ui: &Rc<Ui>,
    state: &Rc<RefCell<AppState>>,
    dialog: &gtk::Window,
) {
    clear_children_list(list);
    let query = query.trim().to_lowercase();
    let mut visible = 0;
    for app in apps.iter() {
        if !query.is_empty()
            && !app.name.to_lowercase().contains(&query)
            && !app.command.join(" ").to_lowercase().contains(&query)
        {
            continue;
        }
        visible += 1;
        let row = gtk::ListBoxRow::new();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
        content.set_margin_top(8);
        content.set_margin_bottom(8);
        content.set_margin_start(10);
        content.set_margin_end(10);
        let title = gtk::Label::new(Some(&app.name));
        title.set_xalign(0.0);
        title.add_css_class("heading");
        let command = gtk::Label::new(Some(&format!("open {}", app.command.join(" "))));
        command.set_xalign(0.0);
        command.add_css_class("dim-label");
        content.append(&title);
        content.append(&command);
        row.set_child(Some(&content));
        let app_for_row = app.clone();
        let ui_for_row = Rc::clone(ui);
        let state_for_row = Rc::clone(state);
        let dialog_for_row = dialog.clone();
        row.connect_activate(move |_| {
            append_app_step(&ui_for_row, &state_for_row, &app_for_row);
            dialog_for_row.close();
        });
        list.append(&row);
    }

    if visible == 0 {
        let empty = gtk::Label::new(Some(if query.is_empty() {
            "No installed desktop apps were found. Use the custom command below."
        } else {
            "No installed apps match this search. Try a different name or use a custom command."
        }));
        empty.set_wrap(true);
        empty.set_margin_top(20);
        empty.set_margin_start(16);
        empty.set_margin_end(16);
        empty.add_css_class("dim-label");
        list.append(&empty);
    }
}

fn show_app_picker(ui: &Rc<Ui>, state: &Rc<RefCell<AppState>>) {
    let dialog = gtk::Window::builder()
        .title("Choose an installed app")
        .transient_for(&ui.window)
        .modal(true)
        .default_width(520)
        .default_height(580)
        .build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_margin_top(18);
    root.set_margin_bottom(18);
    root.set_margin_start(18);
    root.set_margin_end(18);

    let intro = gtk::Label::new(Some(
        "Choose an installed desktop app. Hyusk will add a readable step and preserve its launch command in commands.json.",
    ));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    intro.add_css_class("dim-label");
    root.append(&intro);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search installed apps"));
    search.set_tooltip_text(Some("Filter installed desktop applications"));
    root.append(&search);

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("boxed-list");
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&list)
        .build();
    root.append(&scroll);

    let custom_label = gtk::Label::new(Some("Custom app command"));
    custom_label.set_xalign(0.0);
    custom_label.add_css_class("heading");
    root.append(&custom_label);
    let custom_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let custom_entry = gtk::Entry::new();
    custom_entry.set_hexpand(true);
    custom_entry.set_placeholder_text(Some("e.g. flatpak run com.example.App"));
    custom_entry.set_tooltip_text(Some(
        "Executable and optional arguments for an app not listed above",
    ));
    let custom_button = gtk::Button::with_label("Use command");
    custom_button.add_css_class("suggested-action");
    custom_row.append(&custom_entry);
    custom_row.append(&custom_button);
    root.append(&custom_row);

    let close_button = gtk::Button::with_label("Cancel");
    close_button.set_halign(gtk::Align::End);
    root.append(&close_button);
    dialog.set_child(Some(&root));

    let apps = Rc::new(installed_apps());
    rebuild_app_rows(&list, &apps, "", ui, state, &dialog);
    let list_for_search = list.clone();
    let apps_for_search = Rc::clone(&apps);
    let ui_for_search = Rc::clone(ui);
    let state_for_search = Rc::clone(state);
    let dialog_for_search = dialog.clone();
    search.connect_search_changed(move |entry| {
        rebuild_app_rows(
            &list_for_search,
            &apps_for_search,
            &entry.text(),
            &ui_for_search,
            &state_for_search,
            &dialog_for_search,
        );
    });

    let ui_for_custom = Rc::clone(ui);
    let state_for_custom = Rc::clone(state);
    let dialog_for_custom = dialog.clone();
    custom_button.connect_clicked(move |_| {
        let command = desktop_exec_command(&custom_entry.text());
        if command.is_empty() {
            set_status(
                &ui_for_custom,
                "Enter an executable or flatpak command first.",
                true,
            );
            custom_entry.grab_focus();
            return;
        }
        let app = DesktopApp {
            name: command[0].clone(),
            command,
        };
        append_app_step(&ui_for_custom, &state_for_custom, &app);
        dialog_for_custom.close();
    });
    let dialog_for_close = dialog.clone();
    close_button.connect_clicked(move |_| dialog_for_close.close());
    dialog.present();
}

fn normalize_url(value: &str) -> String {
    let value = value.trim();
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        format!("https://{value}")
    }
}

fn show_website_builder(ui: &Rc<Ui>, state: &Rc<RefCell<AppState>>) {
    let dialog = gtk::Window::builder()
        .title("Add website step")
        .transient_for(&ui.window)
        .modal(true)
        .default_width(480)
        .default_height(300)
        .build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_margin_top(20);
    root.set_margin_bottom(20);
    root.set_margin_start(20);
    root.set_margin_end(20);
    let intro = gtk::Label::new(Some(
        "Choose the URL and browser explicitly so the workflow behaves the same every time.",
    ));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    intro.add_css_class("dim-label");
    root.append(&intro);

    let url_label = gtk::Label::new(Some("Website URL"));
    url_label.set_xalign(0.0);
    url_label.add_css_class("heading");
    root.append(&url_label);
    let url_entry = gtk::Entry::new();
    url_entry.set_placeholder_text(Some("https://example.com"));
    url_entry.set_hexpand(true);
    root.append(&url_entry);

    let browser_label = gtk::Label::new(Some("Browser"));
    browser_label.set_xalign(0.0);
    browser_label.add_css_class("heading");
    root.append(&browser_label);
    let browser = gtk::DropDown::from_strings(&["Default browser", "Firefox", "Brave"]);
    browser.set_selected(0);
    root.append(&browser);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let add = gtk::Button::with_label("Add website step");
    add.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&add);
    root.append(&buttons);
    dialog.set_child(Some(&root));

    let ui_for_add = Rc::clone(ui);
    let state_for_add = Rc::clone(state);
    let dialog_for_add = dialog.clone();
    add.connect_clicked(move |_| {
        let raw_url = url_entry.text();
        if raw_url.trim().is_empty() {
            set_status(&ui_for_add, "Enter a website URL first.", true);
            url_entry.grab_focus();
            return;
        }
        let url = normalize_url(&raw_url);
        let step = match browser.selected() {
            1 => format!("open {url} in firefox"),
            2 => format!("open {url} in brave"),
            _ => format!("go to {url}"),
        };
        add_step_with_value(&ui_for_add, &state_for_add, &step);
        set_status(
            &ui_for_add,
            "Website step added. Save the workflow when ready.",
            false,
        );
        dialog_for_add.close();
    });
    let dialog_for_cancel = dialog.clone();
    cancel.connect_clicked(move |_| dialog_for_cancel.close());
    dialog.present();
}

fn parse_delay_seconds(raw: &str) -> Option<u64> {
    let value = raw.trim().to_lowercase();
    if value.is_empty() {
        return None;
    }
    let (number, multiplier) = if let Some(value) = value.strip_suffix('s') {
        (value, 1)
    } else if let Some(value) = value.strip_suffix('m') {
        (value, 60)
    } else if let Some(value) = value.strip_suffix('h') {
        (value, 60 * 60)
    } else {
        (value.as_str(), 1)
    };
    number
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|seconds| *seconds > 0)
        .map(|seconds| seconds * multiplier)
}

fn selected_workflow_name(state: &Rc<RefCell<AppState>>) -> Option<String> {
    state.borrow().selected.and_then(|index| {
        state
            .borrow()
            .config
            .workflows
            .get(index)
            .map(|workflow| workflow.name.clone())
    })
}

fn schedule_workflow(ui: &Rc<Ui>, state: &Rc<RefCell<AppState>>, delay: &gtk::Entry) {
    let Some(name) = selected_workflow_name(state) else {
        set_status(ui, "Save or select a workflow before scheduling it.", true);
        return;
    };
    let Some(seconds) = parse_delay_seconds(&delay.text()) else {
        set_status(ui, "Enter a delay such as 30s, 15m, or 2h.", true);
        delay.grab_focus();
        return;
    };
    let payload = serde_json::json!({
        "workflow": name,
        "delay_seconds": seconds,
        "once": true,
    });
    match write_schedule_control("schedule_workflow", payload) {
        Ok(()) => set_status(ui, "One-time schedule request sent to Hyusk.", false),
        Err(error) => set_status(ui, &format!("Could not schedule workflow: {error}"), true),
    }
}

fn schedule_reminder(
    ui: &Rc<Ui>,
    state: &Rc<RefCell<AppState>>,
    delay: &gtk::Entry,
    text: &gtk::Entry,
) {
    let Some(seconds) = parse_delay_seconds(&delay.text()) else {
        set_status(ui, "Enter a delay such as 30s, 15m, or 2h.", true);
        delay.grab_focus();
        return;
    };
    let reminder = text.text().trim().to_string();
    if reminder.is_empty() {
        set_status(ui, "Enter what you want Hyusk to remind you about.", true);
        text.grab_focus();
        return;
    }
    let payload = serde_json::json!({
        "text": reminder,
        "delay_seconds": seconds,
        "once": true,
        "workflow": selected_workflow_name(state),
    });
    match write_schedule_control("schedule_reminder", payload) {
        Ok(()) => set_status(ui, "One-time reminder request sent to Hyusk.", false),
        Err(error) => set_status(ui, &format!("Could not schedule reminder: {error}"), true),
    }
}

fn clear_children(container: &gtk::Box) {
    let mut child = container.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        container.remove(&widget);
    }
}

fn rebuild_steps(ui: &Rc<Ui>, state: &Rc<RefCell<AppState>>, values: &[String]) {
    clear_children(&ui.steps_box);
    let total = values.len();

    for (index, value) in values.iter().enumerate() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.add_css_class("step-row");

        let entry = gtk::Entry::new();
        entry.set_text(value);
        entry.set_hexpand(true);
        entry.set_placeholder_text(Some("e.g. open Firefox"));
        row.append(&entry);

        let up = gtk::Button::from_icon_name("go-up-symbolic");
        up.set_tooltip_text(Some("Move step up"));
        up.set_sensitive(index > 0);
        let ui_for_up = Rc::clone(ui);
        let state_for_up = Rc::clone(state);
        up.connect_clicked(move |_| {
            let mut steps = read_steps(&ui_for_up);
            if index > 0 && index < steps.len() {
                steps.swap(index, index - 1);
                rebuild_steps(&ui_for_up, &state_for_up, &steps);
            }
        });
        row.append(&up);

        let down = gtk::Button::from_icon_name("go-down-symbolic");
        down.set_tooltip_text(Some("Move step down"));
        down.set_sensitive(index + 1 < total);
        let ui_for_down = Rc::clone(ui);
        let state_for_down = Rc::clone(state);
        down.connect_clicked(move |_| {
            let mut steps = read_steps(&ui_for_down);
            if index + 1 < steps.len() {
                steps.swap(index, index + 1);
                rebuild_steps(&ui_for_down, &state_for_down, &steps);
            }
        });
        row.append(&down);

        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.set_tooltip_text(Some("Remove step"));
        let ui_for_remove = Rc::clone(ui);
        let state_for_remove = Rc::clone(state);
        remove.connect_clicked(move |_| {
            let mut steps = read_steps(&ui_for_remove);
            if index < steps.len() {
                steps.remove(index);
                rebuild_steps(&ui_for_remove, &state_for_remove, &steps);
            }
        });
        row.append(&remove);

        ui.steps_box.append(&row);
    }
}

fn refresh_editor(ui: &Rc<Ui>, state: &Rc<RefCell<AppState>>) {
    let workflow = state
        .borrow()
        .selected
        .and_then(|index| state.borrow().config.workflows.get(index).cloned());

    match workflow {
        Some(workflow) => {
            ui.editor_title.set_text(&workflow.name);
            ui.name_entry.set_text(&workflow.name);
            ui.phrases_view
                .buffer()
                .set_text(&workflow.phrases.join("\n"));
            rebuild_steps(ui, state, &workflow.steps);
            ui.save_button.set_sensitive(true);
            ui.delete_button.set_sensitive(true);
            ui.run_button.set_sensitive(true);
        }
        None => {
            ui.editor_title.set_text("New workflow");
            ui.name_entry.set_text("");
            ui.phrases_view.buffer().set_text("");
            rebuild_steps(ui, state, &[String::new()]);
            ui.save_button.set_sensitive(true);
            ui.delete_button.set_sensitive(false);
            ui.run_button.set_sensitive(false);
        }
    }
}

fn select_workflow(index: usize, ui: &Rc<Ui>, state: &Rc<RefCell<AppState>>) {
    if index >= state.borrow().config.workflows.len() {
        return;
    }
    state.borrow_mut().selected = Some(index);
    refresh_editor(ui, state);
    set_status(ui, "Workflow loaded", false);
}

fn rebuild_workflow_list(ui: &Rc<Ui>, state: &Rc<RefCell<AppState>>) {
    clear_children_list(&ui.list_box);
    let query = ui.search.text().trim().to_lowercase();
    let selected = state.borrow().selected;
    let workflows = state.borrow().config.workflows.clone();

    for (index, workflow) in workflows.iter().enumerate() {
        let haystack = std::iter::once(workflow.name.as_str())
            .chain(workflow.phrases.iter().map(String::as_str))
            .chain(workflow.steps.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if !query.is_empty() && !haystack.contains(&query) {
            continue;
        }

        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&format!("workflow-row-{index}"));
        row.set_activatable(true);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 3);
        content.set_margin_top(10);
        content.set_margin_bottom(10);
        content.set_margin_start(12);
        content.set_margin_end(12);

        let title = gtk::Label::new(Some(&workflow.name));
        title.set_xalign(0.0);
        title.add_css_class("heading");
        let detail = gtk::Label::new(Some(&format!(
            "{} step{}  ·  {} trigger{}",
            workflow.steps.len(),
            if workflow.steps.len() == 1 { "" } else { "s" },
            workflow.phrases.len(),
            if workflow.phrases.len() == 1 { "" } else { "s" },
        )));
        detail.set_xalign(0.0);
        detail.add_css_class("dim-label");
        content.append(&title);
        content.append(&detail);
        row.set_child(Some(&content));

        let ui_for_row = Rc::clone(ui);
        let state_for_row = Rc::clone(state);
        row.connect_activate(move |_| select_workflow(index, &ui_for_row, &state_for_row));
        ui.list_box.append(&row);

        if selected == Some(index) {
            ui.list_box.select_row(Some(&row));
        }
    }

    if ui.list_box.first_child().is_none() {
        let empty = gtk::Label::new(Some(if query.is_empty() {
            "No workflows yet. Create one to get started."
        } else {
            "No workflows match your search."
        }));
        empty.set_wrap(true);
        empty.set_margin_top(24);
        empty.set_margin_start(18);
        empty.set_margin_end(18);
        empty.add_css_class("dim-label");
        ui.list_box.append(&empty);
    }
}

fn clear_children_list(list: &gtk::ListBox) {
    let mut child = list.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        list.remove(&widget);
    }
}

fn new_workflow(ui: &Rc<Ui>, state: &Rc<RefCell<AppState>>) {
    state.borrow_mut().selected = None;
    ui.list_box.select_row(None::<&gtk::ListBoxRow>);
    refresh_editor(ui, state);
    set_status(ui, "Draft ready", false);
    ui.name_entry.grab_focus();
}

fn save_workflow(ui: &Rc<Ui>, state: &Rc<RefCell<AppState>>) {
    let name = ui.name_entry.text().trim().to_string();
    let phrases = read_phrases(ui);
    let steps = read_steps(ui);
    if name.is_empty() {
        set_status(ui, "Give this workflow a name first.", true);
        ui.name_entry.grab_focus();
        return;
    }
    if steps.is_empty() {
        set_status(ui, "Add at least one step before saving.", true);
        return;
    }

    let selected = state.borrow().selected;
    let mut config = state.borrow().config.clone();
    if config
        .workflows
        .iter()
        .enumerate()
        .any(|(index, workflow)| {
            Some(index) != selected && workflow.name.eq_ignore_ascii_case(&name)
        })
    {
        set_status(ui, "A workflow with that name already exists.", true);
        return;
    }

    let workflow = Workflow {
        name,
        phrases,
        steps,
    };
    let index = match selected {
        Some(index) if index < config.workflows.len() => {
            config.workflows[index] = workflow;
            index
        }
        _ => {
            config.workflows.push(workflow);
            config.workflows.len() - 1
        }
    };

    if let Err(error) = save_config(&config) {
        set_status(ui, &format!("Could not save workflow: {error}"), true);
        return;
    }

    {
        let mut current = state.borrow_mut();
        current.config = config;
        current.selected = Some(index);
    }
    rebuild_workflow_list(ui, state);
    refresh_editor(ui, state);
    set_status(ui, "Workflow saved", false);
}

fn delete_workflow(ui: &Rc<Ui>, state: &Rc<RefCell<AppState>>) {
    let Some(index) = state.borrow().selected else {
        return;
    };
    let Some(workflow) = state.borrow().config.workflows.get(index).cloned() else {
        return;
    };

    let dialog = adw::AlertDialog::new(
        Some("Delete workflow?"),
        Some(&format!(
            "“{}” will be removed from your Hyusk shortcuts.",
            workflow.name
        )),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);

    let ui_for_dialog = Rc::clone(ui);
    let state_for_dialog = Rc::clone(state);
    dialog.connect_response(None, move |dialog, response| {
        if response == "delete" {
            let mut config = state_for_dialog.borrow().config.clone();
            if index < config.workflows.len() {
                config.workflows.remove(index);
                match save_config(&config) {
                    Ok(()) => {
                        let mut current = state_for_dialog.borrow_mut();
                        current.config = config;
                        current.selected = None;
                        drop(current);
                        rebuild_workflow_list(&ui_for_dialog, &state_for_dialog);
                        refresh_editor(&ui_for_dialog, &state_for_dialog);
                        set_status(&ui_for_dialog, "Workflow deleted", false);
                    }
                    Err(error) => set_status(
                        &ui_for_dialog,
                        &format!("Could not delete workflow: {error}"),
                        true,
                    ),
                }
            }
        }
        dialog.close();
    });
    dialog.present(Some(&ui.window));
}

fn run_workflow(ui: &Rc<Ui>, state: &Rc<RefCell<AppState>>) {
    let Some(index) = state.borrow().selected else {
        return;
    };
    let Some(name) = state
        .borrow()
        .config
        .workflows
        .get(index)
        .map(|workflow| workflow.name.clone())
    else {
        return;
    };
    match write_control("run_workflow", &name) {
        Ok(()) => set_status(ui, &format!("Sent “{}” to Hyusk", name), false),
        Err(error) => set_status(ui, &format!("Could not contact Hyusk: {error}"), true),
    }
}

fn add_step(ui: &Rc<Ui>, state: &Rc<RefCell<AppState>>) {
    add_step_with_value(ui, state, "");
}

fn add_step_with_value(ui: &Rc<Ui>, state: &Rc<RefCell<AppState>>, value: &str) {
    let mut steps = read_steps(ui);
    steps.push(value.to_string());
    rebuild_steps(ui, state, &steps);
    let mut child = ui.steps_box.last_child();
    if let Some(row) = child.take() {
        if let Some(entry) = row
            .first_child()
            .and_then(|widget| widget.downcast::<gtk::Entry>().ok())
        {
            entry.grab_focus();
        }
    }
}

fn action_template(index: u32) -> Option<&'static str> {
    match index {
        3 => Some("open https://example.com in brave"),
        4 => Some("switch to Firefox"),
        5 => Some("set timer for 25 minutes"),
        6 => Some("set volume to 30"),
        7 => Some("play"),
        8 => Some("screenshot"),
        9 => Some("remember that "),
        10 => Some("lock the screen"),
        _ => None,
    }
}

fn build_ui(app: &adw::Application) {
    let state = Rc::new(RefCell::new(AppState {
        config: load_config(),
        selected: None,
    }));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Hyusk Workflows")
        .default_width(1040)
        .default_height(700)
        .build();

    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::Single);
    list_box.add_css_class("boxed-list");

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search workflows"));
    search.set_margin_bottom(12);

    let editor_title = gtk::Label::new(Some("New workflow"));
    editor_title.set_xalign(0.0);
    editor_title.add_css_class("title-2");

    let name_entry = gtk::Entry::new();
    name_entry.set_placeholder_text(Some("Focus mode"));
    name_entry.set_hexpand(true);

    let phrases_view = gtk::TextView::new();
    phrases_view.set_wrap_mode(gtk::WrapMode::WordChar);
    phrases_view.set_vexpand(false);
    phrases_view.set_size_request(-1, 76);
    phrases_view.set_tooltip_text(Some(
        "One phrase per line. The workflow name is also a trigger.",
    ));

    let steps_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    steps_box.set_vexpand(false);
    let status = gtk::Label::new(Some("Ready"));
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.add_css_class("dim-label");

    let save_button = gtk::Button::with_label("Save");
    save_button.add_css_class("suggested-action");
    let delete_button = gtk::Button::from_icon_name("user-trash-symbolic");
    delete_button.set_tooltip_text(Some("Delete workflow"));
    let run_button = gtk::Button::with_label("Run");
    run_button.add_css_class("pill");

    let ui = Rc::new(Ui {
        window: window.clone(),
        list_box: list_box.clone(),
        search: search.clone(),
        editor_title: editor_title.clone(),
        name_entry: name_entry.clone(),
        phrases_view: phrases_view.clone(),
        steps_box: steps_box.clone(),
        status: status.clone(),
        save_button: save_button.clone(),
        delete_button: delete_button.clone(),
        run_button: run_button.clone(),
    });

    let new_button = gtk::Button::from_icon_name("document-new-symbolic");
    new_button.set_tooltip_text(Some("New workflow"));
    let add_step_button = gtk::Button::with_label("Add custom step");
    add_step_button.add_css_class("flat");
    let action_picker = gtk::DropDown::from_strings(&[
        "Add an action…",
        "Open an installed app…",
        "Open a website…",
        "Open a website in Brave",
        "Switch window",
        "Start timer",
        "Set volume",
        "Control media",
        "Take screenshot",
        "Save a memory",
        "Lock screen",
    ]);
    action_picker.set_tooltip_text(Some("Choose a common Hyusk action, then edit its details"));

    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar.set_margin_top(18);
    sidebar.set_margin_bottom(12);
    sidebar.set_margin_start(14);
    sidebar.set_margin_end(14);
    sidebar.append(&search);
    let list_scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&list_box)
        .build();
    sidebar.append(&list_scroll);

    let form = gtk::Box::new(gtk::Orientation::Vertical, 18);
    form.set_margin_top(28);
    form.set_margin_bottom(28);
    form.set_margin_start(32);
    form.set_margin_end(32);

    let intro = gtk::Label::new(Some(
        "Build a dependable shortcut from the same commands Hyusk understands by voice.",
    ));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    intro.add_css_class("dim-label");
    form.append(&editor_title);
    form.append(&intro);

    let name_label = gtk::Label::new(Some("Name"));
    name_label.set_xalign(0.0);
    name_label.add_css_class("heading");
    form.append(&name_label);
    form.append(&name_entry);

    let phrase_label = gtk::Label::new(Some("Voice triggers"));
    phrase_label.set_xalign(0.0);
    phrase_label.add_css_class("heading");
    let phrase_hint = gtk::Label::new(Some(
        "Optional aliases, one per line. Example: start my focus session",
    ));
    phrase_hint.set_xalign(0.0);
    phrase_hint.add_css_class("dim-label");
    form.append(&phrase_label);
    form.append(&phrase_hint);
    form.append(&phrases_view);

    let schedule_label = gtk::Label::new(Some("One-time scheduling"));
    schedule_label.set_xalign(0.0);
    schedule_label.add_css_class("heading");
    let schedule_hint = gtk::Label::new(Some(
        "Send a delayed run or reminder request to a Hyusk service with schedule support. Delays accept 30s, 15m, or 2h.",
    ));
    schedule_hint.set_xalign(0.0);
    schedule_hint.set_wrap(true);
    schedule_hint.add_css_class("dim-label");
    form.append(&schedule_label);
    form.append(&schedule_hint);

    let schedule_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let schedule_delay = gtk::Entry::new();
    schedule_delay.set_width_chars(8);
    schedule_delay.set_placeholder_text(Some("15m"));
    schedule_delay.set_tooltip_text(Some("Delay before the one-time action, such as 15m or 2h"));
    let schedule_button = gtk::Button::with_label("Schedule run");
    let reminder_entry = gtk::Entry::new();
    reminder_entry.set_hexpand(true);
    reminder_entry.set_placeholder_text(Some("Reminder text (optional)"));
    reminder_entry.set_tooltip_text(Some("Text to remind you about after the same delay"));
    let reminder_button = gtk::Button::with_label("Schedule reminder");
    schedule_row.append(&schedule_delay);
    schedule_row.append(&schedule_button);
    schedule_row.append(&reminder_entry);
    schedule_row.append(&reminder_button);
    form.append(&schedule_row);

    let steps_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let steps_label = gtk::Label::new(Some("Steps"));
    steps_label.set_xalign(0.0);
    steps_label.add_css_class("heading");
    steps_header.append(&steps_label);
    let steps_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    steps_spacer.set_hexpand(true);
    steps_header.append(&steps_spacer);
    steps_header.append(&action_picker);
    steps_header.append(&add_step_button);
    form.append(&steps_header);
    form.append(&steps_box);
    form.append(&status);

    let form_scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&form)
        .build();

    let header = adw::HeaderBar::new();
    header.pack_start(&new_button);
    header.pack_end(&delete_button);
    header.pack_end(&run_button);
    header.pack_end(&save_button);

    let detail_toolbar = adw::ToolbarView::new();
    detail_toolbar.add_top_bar(&header);
    detail_toolbar.set_content(Some(&form_scroll));

    let sidebar_page = adw::NavigationPage::builder()
        .title("Workflows")
        .child(&sidebar)
        .build();
    let detail_page = adw::NavigationPage::builder()
        .title("Edit workflow")
        .child(&detail_toolbar)
        .build();
    let split = adw::NavigationSplitView::new();
    split.set_sidebar(Some(&sidebar_page));
    split.set_content(Some(&detail_page));
    split.set_min_sidebar_width(260.0);
    split.set_max_sidebar_width(360.0);
    window.set_content(Some(&split));

    let state_for_search = Rc::clone(&state);
    let ui_for_search = Rc::clone(&ui);
    search
        .connect_search_changed(move |_| rebuild_workflow_list(&ui_for_search, &state_for_search));

    let state_for_rows = Rc::clone(&state);
    let ui_for_rows = Rc::clone(&ui);
    list_box.connect_row_selected(move |_, row| {
        let Some(row) = row else { return };
        let name = row.widget_name();
        let Some(index) = name
            .strip_prefix("workflow-row-")
            .and_then(|value| value.parse().ok())
        else {
            return;
        };
        select_workflow(index, &ui_for_rows, &state_for_rows);
    });

    let state_for_new = Rc::clone(&state);
    let ui_for_new = Rc::clone(&ui);
    new_button.connect_clicked(move |_| new_workflow(&ui_for_new, &state_for_new));

    let state_for_add = Rc::clone(&state);
    let ui_for_add = Rc::clone(&ui);
    add_step_button.connect_clicked(move |_| add_step(&ui_for_add, &state_for_add));

    let state_for_picker = Rc::clone(&state);
    let ui_for_picker = Rc::clone(&ui);
    action_picker.connect_selected_notify(move |picker| {
        let selected = picker.selected();
        match selected {
            1 => show_app_picker(&ui_for_picker, &state_for_picker),
            2 => show_website_builder(&ui_for_picker, &state_for_picker),
            _ => {
                if let Some(template) = action_template(selected) {
                    add_step_with_value(&ui_for_picker, &state_for_picker, template);
                }
            }
        }
        picker.set_selected(0);
    });

    let state_for_schedule = Rc::clone(&state);
    let ui_for_schedule = Rc::clone(&ui);
    let schedule_delay_for_run = schedule_delay.clone();
    schedule_button.connect_clicked(move |_| {
        schedule_workflow(
            &ui_for_schedule,
            &state_for_schedule,
            &schedule_delay_for_run,
        )
    });

    let state_for_reminder = Rc::clone(&state);
    let ui_for_reminder = Rc::clone(&ui);
    let schedule_delay_for_reminder = schedule_delay.clone();
    reminder_button.connect_clicked(move |_| {
        schedule_reminder(
            &ui_for_reminder,
            &state_for_reminder,
            &schedule_delay_for_reminder,
            &reminder_entry,
        )
    });

    let state_for_save = Rc::clone(&state);
    let ui_for_save = Rc::clone(&ui);
    save_button.connect_clicked(move |_| save_workflow(&ui_for_save, &state_for_save));

    let state_for_delete = Rc::clone(&state);
    let ui_for_delete = Rc::clone(&ui);
    delete_button.connect_clicked(move |_| delete_workflow(&ui_for_delete, &state_for_delete));

    let state_for_run = Rc::clone(&state);
    let ui_for_run = Rc::clone(&ui);
    run_button.connect_clicked(move |_| run_workflow(&ui_for_run, &state_for_run));

    rebuild_steps(&ui, &state, &[String::new()]);
    rebuild_workflow_list(&ui, &state);
    window.present();
}

fn main() {
    let app = adw::Application::builder()
        .application_id("io.github.hyusk.Workflows")
        .build();
    app.connect_activate(build_ui);
    app.run();
}
