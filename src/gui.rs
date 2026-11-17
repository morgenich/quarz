use crate::PasswordNeeded;
use crate::lang::{self, Lang, tr, trf};
use crate::ops::{self, CreateOptions, Listing};
use crate::progress::Progress;
use crate::unpack::Overwrite;
use eframe::egui::{self, Color32, FontFamily, RichText};
use egui_extras::{Column, TableBuilder};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;

const I_FOLDER: &str = "\u{E8B7}";
const I_FILE: &str = "\u{E7C3}";
const I_ARCHIVE: &str = "\u{F012}";
const I_ADD: &str = "\u{E710}";
const I_DELETE: &str = "\u{E74D}";
const I_BACK: &str = "\u{E72B}";
const I_EXTRACT: &str = "\u{E896}";
const I_OPEN: &str = "\u{E8E5}";
const I_CHECK: &str = "\u{E73E}";
const I_MORE: &str = "\u{E712}";
const I_WARNING: &str = "\u{E7BA}";
const I_LOCK: &str = "\u{E72E}";
const I_CHEVRON: &str = "\u{E76C}";
const I_SHIELD: &str = "\u{EA18}";
const I_GLOBE: &str = "\u{E774}";

pub enum Start {
    Home,
    Open(PathBuf),
    PackDialog(Vec<PathBuf>),
    PackNow(Vec<PathBuf>),
    ExtractDialog(PathBuf),
    ExtractHere(PathBuf),
    ExtractToFolder(PathBuf),
    SelfExtract(PathBuf),
}

#[derive(Clone, Copy)]
struct Palette {
    accent: Color32,
    on_accent: Color32,
    bg: Color32,
    card: Color32,
    border: Color32,
    text: Color32,
    muted: Color32,
    hover: Color32,
    danger: Color32,
    good: Color32,
    folder: Color32,
}

fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            accent: Color32::from_rgb(76, 194, 255),
            on_accent: Color32::from_rgb(0, 20, 34),
            bg: Color32::from_rgb(32, 32, 32),
            card: Color32::from_rgb(43, 43, 43),
            border: Color32::from_rgb(62, 62, 62),
            text: Color32::from_rgb(245, 245, 245),
            muted: Color32::from_rgb(170, 170, 170),
            hover: Color32::from_rgb(56, 56, 56),
            danger: Color32::from_rgb(255, 122, 122),
            good: Color32::from_rgb(108, 203, 95),
            folder: Color32::from_rgb(240, 190, 70),
        }
    } else {
        Palette {
            accent: Color32::from_rgb(0, 103, 192),
            on_accent: Color32::WHITE,
            bg: Color32::from_rgb(243, 243, 243),
            card: Color32::WHITE,
            border: Color32::from_rgb(222, 222, 222),
            text: Color32::from_rgb(27, 27, 27),
            muted: Color32::from_rgb(96, 96, 96),
            hover: Color32::from_rgb(234, 240, 247),
            danger: Color32::from_rgb(196, 43, 28),
            good: Color32::from_rgb(15, 123, 15),
            folder: Color32::from_rgb(222, 158, 30),
        }
    }
}

fn colors(ui: &egui::Ui) -> Palette {
    palette(ui.visuals().dark_mode)
}

fn bold(text: impl Into<String>) -> RichText {
    RichText::new(text).family(FontFamily::Name("bold".into()))
}

pub fn setup(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let dir = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
    let mut load = |name: &str, files: &[&str], tweak: egui::FontTweak| -> bool {
        for file in files {
            if let Ok(bytes) = std::fs::read(dir.join(file)) {
                fonts.font_data.insert(name.to_string(), Arc::new(egui::FontData::from_owned(bytes).tweak(tweak)));
                return true;
            }
        }
        false
    };
    let regular = load("ui", &["segoeui.ttf"], egui::FontTweak::default());
    let strong = load("ui-bold", &["seguisb.ttf", "segoeuib.ttf"], egui::FontTweak::default());
    let icons = load(
        "icons",
        &["SegoeIcons.ttf", "segmdl2.ttf"],
        egui::FontTweak { y_offset_factor: 0.14, ..Default::default() },
    );
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let chain = |first: Option<&str>| {
        let mut names: Vec<String> = first.into_iter().map(String::from).collect();
        if icons {
            names.push("icons".into());
        }
        names.extend(fallback.iter().cloned());
        names
    };
    fonts.families.insert(FontFamily::Proportional, chain(regular.then_some("ui")));
    fonts
        .families
        .insert(FontFamily::Name("bold".into()), chain(if strong { Some("ui-bold") } else { regular.then_some("ui") }));
    ctx.set_fonts(fonts);

    for (theme, dark) in [(egui::Theme::Light, false), (egui::Theme::Dark, true)] {
        let p = palette(dark);
        ctx.style_mut_of(theme, |style| {
            style.interaction.selectable_labels = false;
            style.spacing.item_spacing = egui::vec2(8.0, 8.0);
            style.spacing.button_padding = egui::vec2(14.0, 7.0);
            style.spacing.interact_size.y = 32.0;
            style.text_styles = [
                (egui::TextStyle::Heading, egui::FontId::new(21.0, FontFamily::Name("bold".into()))),
                (egui::TextStyle::Body, egui::FontId::new(15.0, FontFamily::Proportional)),
                (egui::TextStyle::Button, egui::FontId::new(15.0, FontFamily::Proportional)),
                (egui::TextStyle::Small, egui::FontId::new(12.5, FontFamily::Proportional)),
                (egui::TextStyle::Monospace, egui::FontId::new(14.0, FontFamily::Monospace)),
            ]
            .into();
            let v = &mut style.visuals;
            v.panel_fill = p.bg;
            v.window_fill = p.card;
            v.extreme_bg_color = p.card;
            v.window_stroke = egui::Stroke::new(1.0, p.border);
            v.window_corner_radius = egui::CornerRadius::same(10);
            v.menu_corner_radius = egui::CornerRadius::same(8);
            v.selection.bg_fill = p.accent.gamma_multiply(if dark { 0.35 } else { 0.18 });
            v.selection.stroke = egui::Stroke::new(1.0, p.accent);
            v.hyperlink_color = p.accent;
            v.widgets.noninteractive.fg_stroke.color = p.text;
            v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.border);
            v.widgets.inactive.fg_stroke.color = p.text;
            v.widgets.inactive.weak_bg_fill = p.card;
            v.widgets.inactive.bg_fill = p.card;
            v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, p.border);
            v.widgets.hovered.weak_bg_fill = p.hover;
            v.widgets.hovered.bg_fill = p.hover;
            v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, p.accent);
            v.widgets.hovered.fg_stroke.color = p.text;
            v.widgets.active.weak_bg_fill = p.hover;
            v.widgets.active.bg_fill = p.hover;
            v.widgets.active.fg_stroke.color = p.text;
            v.widgets.open.weak_bg_fill = p.hover;
            for w in [
                &mut v.widgets.noninteractive,
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.open,
            ] {
                w.corner_radius = egui::CornerRadius::same(6);
                w.expansion = 0.0;
            }
        });
    }
}

#[derive(Clone)]
struct Row {
    name: String,
    size: u64,
    mtime: i64,
    is_dir: bool,
    key: String,
}

struct View {
    path: PathBuf,
    inner: String,
    listing: Arc<Listing>,
    password: Option<String>,
    rows: Vec<Row>,
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
}

enum Screen {
    Home,
    Archive(View),
}

pub struct Outcome {
    pub title: String,
    pub lines: Vec<String>,
    pub ok: bool,
    open: Option<PathBuf>,
    reveal: Option<PathBuf>,
    repair: Option<PathBuf>,
}

impl Outcome {
    fn done(title: &'static str, lines: Vec<String>) -> Outcome {
        Outcome { title: tr(title).into(), lines, ok: true, open: None, reveal: None, repair: None }
    }

    fn from(title: &'static str, result: anyhow::Result<Vec<String>>) -> Outcome {
        match result {
            Ok(lines) => Outcome::done(title, lines),
            Err(e) => Outcome::failed(format!("{e:#}")),
        }
    }

    fn failed(text: String) -> Outcome {
        let title = if text == "abgebrochen" { tr("Abgebrochen") } else { tr("Das hat nicht geklappt") };
        let lines = if text == "abgebrochen" { Vec::new() } else { vec![lang::error(&text)] };
        Outcome { title: title.into(), lines, ok: false, open: None, reveal: None, repair: None }
    }
}

struct Job {
    label: String,
    progress: Arc<Progress>,
    handle: Option<JoinHandle<Outcome>>,
    quit_after: bool,
    shown: f32,
}

struct PackForm {
    inputs: Vec<PathBuf>,
    name: String,
    folder: PathBuf,
    level: u8,
    password: String,
    show_password: bool,
    more: bool,
    split: String,
    split_gb: bool,
    recovery: u8,
    sfx: bool,
    comment: String,
}

struct ExtractForm {
    archive: PathBuf,
    password: Option<String>,
    patterns: Vec<String>,
    strip: usize,
    dest: String,
    overwrite: Overwrite,
    what: String,
    open_after: bool,
    quit_after: bool,
}

#[derive(Clone)]
enum Pending {
    Browse,
    Extract,
    ExtractNow(PathBuf),
}

struct PasswordForm {
    archive: PathBuf,
    text: String,
    wrong: bool,
    then: Pending,
}

struct DetailsForm {
    comment: String,
    recovery: u8,
}

enum Dialog {
    None,
    Pack(PackForm),
    Extract(ExtractForm),
    Password(PasswordForm),
    Details(DetailsForm),
    ConfirmDelete(Vec<String>),
    Result(Outcome),
}

#[derive(PartialEq, Clone, Copy)]
enum Choice {
    Stay,
    Confirm,
    Cancel,
}

pub struct App {
    screen: Screen,
    dialog: Dialog,
    job: Option<Job>,
    passwords: HashMap<PathBuf, String>,
    recent: Vec<PathBuf>,
    compact: bool,
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

fn open_with_default(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("cmd").args(["/C", "start", ""]).arg(path).creation_flags(0x0800_0000).spawn();
    }
    #[cfg(not(windows))]
    let _ = path;
}

fn reveal(path: &Path) {
    let mut command = std::process::Command::new("explorer.exe");
    if path.is_dir() {
        command.arg(path);
    } else {
        command.arg(format!("/select,{}", path.display()));
    }
    let _ = command.spawn();
}

fn recent_file() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("Quarz").join("zuletzt.txt"))
}

fn load_recent() -> Vec<PathBuf> {
    let Some(file) = recent_file() else { return Vec::new() };
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .take(5)
        .collect()
}

fn remember(recent: &mut Vec<PathBuf>, archive: &Path) {
    if std::env::var_os("QUARZ_NO_RECENT").is_some() {
        return;
    }
    recent.retain(|p| p != archive);
    recent.insert(0, archive.to_path_buf());
    recent.truncate(5);
    if let Some(file) = recent_file() {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text: Vec<String> = recent.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let _ = std::fs::write(file, text.join("\n"));
    }
}

fn archive_rows(listing: &Listing, inner: &str) -> Vec<Row> {
    let prefix = if inner.is_empty() { String::new() } else { format!("{inner}/") };
    let mut dirs: HashMap<String, Row> = HashMap::new();
    let mut rows = Vec::new();
    for entry in &listing.entries {
        let Some(rest) = entry.path.strip_prefix(&prefix) else { continue };
        if rest.is_empty() {
            continue;
        }
        match rest.split_once('/') {
            Some((dir, _)) => {
                let row = dirs.entry(dir.to_lowercase()).or_insert_with(|| Row {
                    name: dir.to_string(),
                    size: 0,
                    mtime: 0,
                    is_dir: true,
                    key: format!("{prefix}{dir}"),
                });
                row.size += entry.size;
            }
            None if entry.is_dir => {
                dirs.entry(rest.to_lowercase()).or_insert_with(|| Row {
                    name: rest.to_string(),
                    size: 0,
                    mtime: entry.mtime,
                    is_dir: true,
                    key: entry.path.clone(),
                });
            }
            None => rows.push(Row {
                name: rest.to_string(),
                size: entry.size,
                mtime: entry.mtime,
                is_dir: false,
                key: entry.path.clone(),
            }),
        }
    }
    rows.extend(dirs.into_values());
    rows.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    rows
}

fn unique_name(dir: &Path, stem: &str, ext: &str) -> String {
    let mut name = format!("{stem}.{ext}");
    let mut n = 2;
    while dir.join(&name).exists() {
        name = format!("{stem} ({n}).{ext}");
        n += 1;
    }
    name
}

fn default_dest(archive: &Path) -> PathBuf {
    let base = crate::volume::parts_of(archive).map(|(base, _)| base).unwrap_or_else(|| archive.to_path_buf());
    let mut stem = base.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    if let Some(short) = stem.strip_suffix(".tar") {
        stem = short.to_string();
    }
    let mut dest = base.with_file_name(&stem);
    let mut n = 2;
    while dest.is_file() {
        dest = base.with_file_name(format!("{stem} ({n})"));
        n += 1;
    }
    dest
}

fn suggest_archive(inputs: &[PathBuf]) -> (PathBuf, String) {
    let folder = inputs.first().and_then(|p| p.parent()).map(Path::to_path_buf).unwrap_or_default();
    let stem = match inputs {
        [one] => one.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
        _ => folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Archiv".into()),
    };
    let name = unique_name(&folder, &stem, "qz");
    (folder, name)
}

fn describe_inputs(inputs: &[PathBuf]) -> String {
    match inputs {
        [] => tr("nichts ausgewählt").into(),
        [one] => file_name(one),
        [first, rest @ ..] => trf("{name} und {n} weitere", &[("name", &file_name(first)), ("n", &rest.len().to_string())]),
    }
}

fn summary(first: String, failed: Vec<String>) -> Vec<String> {
    let mut lines = vec![first];
    if !failed.is_empty() {
        lines.push(trf("{n} Dateien mit Fehlern:", &[("n", &failed.len().to_string())]));
        lines.extend(failed);
    }
    lines
}

fn facts(listing: &Listing, with_level: bool) -> Vec<String> {
    let mut facts = Vec::new();
    if listing.kind != "Quarz" {
        facts.push(listing.kind.clone());
    }
    if with_level {
        if let Some(level) = listing.level {
            facts.push(trf("Kompression: {name}", &[("name", lang::level_name(level))]));
        }
    }
    if listing.encrypted {
        facts.push(tr("verschlüsselt").into());
    }
    if listing.parts > 1 {
        facts.push(trf("{n} Teile", &[("n", &listing.parts.to_string())]));
    }
    if listing.protection > 0 {
        facts.push(trf("Schutz {p} %", &[("p", &listing.protection.to_string())]));
    }
    if listing.self_extracting {
        facts.push(tr("selbstentpackend").into());
    }
    if listing.locked {
        facts.push(tr("gesperrt").into());
    }
    facts
}

fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let p = colors(ui);
    let button = egui::Button::new(bold(text).color(p.on_accent)).fill(p.accent).stroke(egui::Stroke::NONE).min_size(egui::vec2(96.0, 34.0));
    ui.add(button)
}

fn muted(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    let color = colors(ui).muted;
    ui.label(RichText::new(text).color(color))
}

fn tile(ui: &mut egui::Ui, icon: &str, title: &str, subtitle: &str, size: egui::Vec2) -> egui::Response {
    let p = colors(ui);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let fill = if response.hovered() { p.hover } else { p.card };
    let border = if response.hovered() { p.accent } else { p.border };
    ui.painter().rect(rect, 8, fill, egui::Stroke::new(1.0, border), egui::StrokeKind::Inside);
    let center = rect.center();
    ui.painter().text(center - egui::vec2(0.0, 24.0), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(26.0), p.accent);
    ui.painter().text(
        center + egui::vec2(0.0, 8.0),
        egui::Align2::CENTER_CENTER,
        title,
        egui::FontId::new(15.0, FontFamily::Name("bold".into())),
        p.text,
    );
    ui.painter().text(center + egui::vec2(0.0, 29.0), egui::Align2::CENTER_CENTER, subtitle, egui::FontId::proportional(12.5), p.muted);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str, &str)], width: f32) {
    let p = colors(ui);
    ui.horizontal(|ui| {
        for (option, title, hint) in options {
            let chosen = *value == *option;
            let text = if chosen { bold(*title).color(p.on_accent) } else { RichText::new(*title) };
            let mut button = egui::Button::new(text).min_size(egui::vec2(width, 34.0));
            if chosen {
                button = button.fill(p.accent).stroke(egui::Stroke::NONE);
            }
            let mut response = ui.add(button);
            if !hint.is_empty() {
                response = response.on_hover_text(*hint);
            }
            if response.clicked() {
                *value = *option;
            }
        }
    });
}

fn progress_bar(ui: &mut egui::Ui, fraction: f32) {
    let p = colors(ui);
    let (track, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 8.0), egui::Sense::hover());
    ui.painter().rect_filled(track, 4, p.border);
    let mut filled = track;
    filled.set_width((track.width() * fraction.clamp(0.0, 1.0)).max(8.0));
    ui.painter().rect_filled(filled, 4, p.accent);
}

fn confirm_row(ui: &mut egui::Ui, confirm: &str, enabled: bool) -> Choice {
    let mut choice = Choice::Stay;
    ui.add_space(6.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_enabled_ui(enabled, |ui| {
            if primary_button(ui, confirm).clicked() {
                choice = Choice::Confirm;
            }
        });
        if ui.button(tr("Abbrechen")).clicked() {
            choice = Choice::Cancel;
        }
    });
    choice
}

fn present<R>(compact: bool, ui: &mut egui::Ui, id: &str, width: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> (R, bool) {
    if compact {
        return (add(ui), false);
    }
    let p = colors(ui);
    let frame = egui::Frame::new()
        .fill(p.card)
        .stroke(egui::Stroke::new(1.0, p.border))
        .corner_radius(12)
        .inner_margin(egui::Margin::same(22))
        .shadow(ui.visuals().window_shadow);
    let modal = egui::Modal::new(egui::Id::unique(id)).frame(frame).show(ui.ctx(), |ui| {
        ui.set_width(width);
        add(ui)
    });
    let dismissed = modal.should_close();
    (modal.inner, dismissed)
}

impl App {
    pub fn new(start: Start) -> App {
        let mut app = App {
            screen: Screen::Home,
            dialog: Dialog::None,
            job: None,
            passwords: HashMap::new(),
            recent: load_recent(),
            compact: false,
        };
        match start {
            Start::Home => {}
            Start::Open(path) => app.with_archive(&path, Pending::Browse),
            Start::PackDialog(inputs) => {
                app.compact = true;
                app.open_pack_form(inputs);
            }
            Start::PackNow(inputs) => {
                app.compact = true;
                let (folder, name) = suggest_archive(&inputs);
                app.start_create(PackForm::new(inputs, folder, name), true);
            }
            Start::ExtractDialog(archive) => {
                app.compact = true;
                app.with_archive(&archive, Pending::Extract);
            }
            Start::ExtractHere(archive) => {
                app.compact = true;
                let dest = archive.parent().map(Path::to_path_buf).unwrap_or_default();
                app.with_archive(&archive, Pending::ExtractNow(dest));
            }
            Start::ExtractToFolder(archive) => {
                app.compact = true;
                let dest = default_dest(&archive);
                app.with_archive(&archive, Pending::ExtractNow(dest));
            }
            Start::SelfExtract(archive) => {
                app.compact = true;
                app.with_archive(&archive, Pending::Extract);
                if let Dialog::Extract(form) = &mut app.dialog {
                    form.dest = archive.with_extension("").to_string_lossy().into_owned();
                    form.what = tr("Selbstentpackendes Archiv").into();
                }
            }
        }
        app
    }

    pub fn window_size(&self) -> [f32; 2] {
        if !self.compact {
            return [960.0, 660.0];
        }
        match self.dialog {
            Dialog::Pack(_) => [560.0, 480.0],
            Dialog::Extract(_) => [560.0, 300.0],
            _ => [500.0, 170.0],
        }
    }

    pub fn preview_job(&mut self, label: &str, fraction: f32) {
        let progress = Arc::new(Progress::default());
        progress.total.store(1000, Ordering::Relaxed);
        progress.done.store((fraction * 1000.0) as u64, Ordering::Relaxed);
        self.job = Some(Job { label: label.into(), progress, handle: None, quit_after: false, shown: 0.0 });
    }

    pub fn preview_result(&mut self, outcome: Outcome) {
        self.dialog = Dialog::Result(outcome);
    }

    pub fn preview_outcome(title: &str, lines: &[&str], ok: bool, reveal: Option<PathBuf>) -> Outcome {
        Outcome { title: title.into(), lines: lines.iter().map(|s| s.to_string()).collect(), ok, open: None, reveal, repair: None }
    }

    pub fn preview_recent(&mut self, recent: Vec<PathBuf>) {
        self.recent = recent;
    }

    pub fn preview_pack(&mut self, inputs: Vec<PathBuf>, more: bool) {
        self.open_pack_form(inputs);
        if let Dialog::Pack(form) = &mut self.dialog {
            form.more = more;
        }
    }

    pub fn preview_compact(&mut self, compact: bool) {
        self.compact = compact;
    }

    pub fn preview_select(&mut self, rows: &[usize]) {
        if let Screen::Archive(view) = &mut self.screen {
            view.selected = rows.iter().copied().collect();
        }
    }

    pub fn preview_enter(&mut self, folder: &str) {
        if let Screen::Archive(view) = &mut self.screen {
            view.enter(folder.to_string());
        }
    }

    pub fn preview_extract(&mut self, dest: Option<&str>) {
        self.action_extract();
        if let (Dialog::Extract(form), Some(dest)) = (&mut self.dialog, dest) {
            form.dest = dest.to_string();
        }
    }

    pub fn preview_details(&mut self) {
        self.action_details();
    }

    pub fn preview_password(&mut self, archive: PathBuf, wrong: bool) {
        self.dialog = Dialog::Password(PasswordForm { archive, text: String::new(), wrong, then: Pending::Browse });
    }

    pub fn busy(&self) -> bool {
        self.job.as_ref().is_some_and(|j| j.handle.is_some())
    }

    pub fn has_archive(&self) -> bool {
        matches!(self.screen, Screen::Archive(_))
    }

    pub fn start_test(&mut self) {
        self.action_test();
    }

    pub fn press_repair(&mut self) -> bool {
        let Dialog::Result(outcome) = &self.dialog else { return false };
        let Some(archive) = outcome.repair.clone() else { return false };
        self.dialog = Dialog::None;
        self.repair(archive);
        true
    }

    pub fn last_result(&self) -> Option<&Outcome> {
        match &self.dialog {
            Dialog::Result(outcome) => Some(outcome),
            _ => None,
        }
    }

    fn with_archive(&mut self, archive: &Path, then: Pending) {
        let password = self.passwords.get(archive).cloned();
        match ops::listing(archive, password.as_deref()) {
            Ok(listing) => self.archive_ready(archive, Arc::new(listing), password, then),
            Err(e) if e.downcast_ref::<PasswordNeeded>().is_some() => {
                self.dialog = Dialog::Password(PasswordForm { archive: archive.to_path_buf(), text: String::new(), wrong: false, then });
            }
            Err(e) if format!("{e:#}").contains("falsches Passwort") => {
                self.passwords.remove(archive);
                self.dialog = Dialog::Password(PasswordForm { archive: archive.to_path_buf(), text: String::new(), wrong: true, then });
            }
            Err(e) => {
                let mut outcome = Outcome::failed(format!("{e:#}"));
                if ops::check_recovery(archive, &Progress::default()).ok().flatten().is_some() {
                    outcome.lines.push(tr("Das Archiv enthält Schutzdaten und lässt sich wahrscheinlich reparieren.").into());
                    outcome.repair = Some(archive.to_path_buf());
                }
                self.dialog = Dialog::Result(outcome);
            }
        }
    }

    fn archive_ready(&mut self, archive: &Path, listing: Arc<Listing>, password: Option<String>, then: Pending) {
        let form = |dest: PathBuf, quit_after: bool| ExtractForm {
            archive: archive.to_path_buf(),
            password: password.clone(),
            patterns: Vec::new(),
            strip: 0,
            dest: dest.to_string_lossy().into_owned(),
            overwrite: Overwrite::Skip,
            what: tr("Alles").into(),
            open_after: true,
            quit_after,
        };
        match then {
            Pending::Browse => {
                remember(&mut self.recent, archive);
                let mut view = View {
                    path: archive.to_path_buf(),
                    inner: String::new(),
                    listing,
                    password,
                    rows: Vec::new(),
                    selected: BTreeSet::new(),
                    anchor: None,
                };
                view.refresh();
                self.screen = Screen::Archive(view);
            }
            Pending::Extract => self.dialog = Dialog::Extract(form(default_dest(archive), self.compact)),
            Pending::ExtractNow(dest) => {
                let mut form = form(dest, true);
                form.open_after = false;
                self.start_extract(form);
            }
        }
    }

    fn start_job(&mut self, label: String, quit_after: bool, work: impl FnOnce(&Progress) -> Outcome + Send + 'static) {
        let progress = Arc::new(Progress::default());
        let shared = progress.clone();
        let handle = std::thread::spawn(move || work(&shared));
        self.job = Some(Job { label, progress, handle: Some(handle), quit_after, shown: 0.0 });
    }

    fn start_extract(&mut self, form: ExtractForm) {
        let quit_after = form.quit_after;
        let label = trf("Entpacke {name}", &[("name", &file_name(&form.archive))]);
        self.start_job(label, quit_after, move |progress| {
            let dest = PathBuf::from(form.dest.trim());
            let result = ops::extract_any(&form.archive, form.password.as_deref(), Some(&dest), &form.patterns, form.overwrite, form.strip, progress)
                .map(|failed| {
                    let total = lang::size(progress.total.load(Ordering::Relaxed));
                    summary(trf("{size} entpackt.", &[("size", &total)]), failed)
                });
            let mut outcome = Outcome::from("Fertig entpackt", result);
            if outcome.ok && outcome.lines.len() > 1 {
                outcome.ok = false;
                outcome.title = tr("Mit Fehlern entpackt").into();
            }
            if outcome.ok {
                outcome.lines.push(trf("Ziel: {path}", &[("path", &dest.to_string_lossy())]));
                if form.open_after {
                    reveal(&dest);
                } else {
                    outcome.reveal = Some(dest);
                }
            }
            outcome
        });
    }

    fn start_create(&mut self, form: PackForm, quit_after: bool) {
        let archive = form.folder.join(form.name.trim());
        let volume_size = match form.split.trim() {
            "" => None,
            text => match ops::parse_size(&format!("{text}{}", if form.split_gb { "g" } else { "m" })) {
                Ok(size) => Some(size),
                Err(e) => {
                    self.dialog = Dialog::Result(Outcome::failed(format!("{e:#}")));
                    return;
                }
            },
        };
        let options = CreateOptions {
            level: form.level,
            threads: ops::default_threads(),
            password: (!form.password.is_empty()).then(|| form.password.clone()),
            comment: form.comment.clone(),
            volume_size,
            recovery: form.recovery,
            sfx: if form.sfx { std::env::current_exe().ok() } else { None },
        };
        let label = trf("Packe {name}", &[("name", &file_name(&archive))]);
        self.start_job(label, quit_after, move |progress| {
            let result = ops::create(&archive, &form.inputs, &options, progress).map(|stats| {
                let first = if options.volume_size.is_some() { crate::volume::part_name(&archive, 1) } else { archive.clone() };
                let size = ops::open_io(&first).map(|(_, total, _)| total).unwrap_or(0);
                let saved = (100.0 - 100.0 * size as f64 / stats.bytes_in.max(1) as f64).max(0.0);
                let mut lines = vec![trf(
                    "{n} Dateien, {from} → {to} ({p} % kleiner)",
                    &[
                        ("n", &stats.files.to_string()),
                        ("from", &lang::size(stats.bytes_in)),
                        ("to", &lang::size(size)),
                        ("p", &format!("{saved:.0}")),
                    ],
                )];
                lines.extend(stats.skipped.iter().map(|s| trf("Übersprungen: {name}", &[("name", s)])));
                (lines, first)
            });
            match result {
                Ok((lines, first)) => Outcome { reveal: Some(first), ..Outcome::done("Fertig gepackt", lines) },
                Err(e) => Outcome::failed(format!("{e:#}")),
            }
        });
    }

    fn start_add(&mut self, inputs: Vec<PathBuf>) {
        let Screen::Archive(view) = &self.screen else { return };
        let (archive, password) = (view.path.clone(), view.password.clone());
        let label = trf("Füge zu {name} hinzu", &[("name", &file_name(&archive))]);
        self.start_job(label, false, move |progress| {
            let result = ops::add(&archive, &inputs, password.as_deref(), None, ops::default_threads(), progress).map(|stats| {
                let values = [("n", stats.files.to_string()), ("size", lang::size(stats.bytes_in))];
                let mut lines = vec![trf("{n} Dateien hinzugefügt ({size}).", &[("n", &values[0].1), ("size", &values[1].1)])];
                lines.extend(stats.skipped.iter().map(|s| trf("Übersprungen: {name}", &[("name", s)])));
                lines
            });
            Outcome::from("Hinzugefügt", result)
        });
    }

    fn open_pack_form(&mut self, inputs: Vec<PathBuf>) {
        if inputs.is_empty() {
            return;
        }
        let (folder, name) = suggest_archive(&inputs);
        self.dialog = Dialog::Pack(PackForm::new(inputs, folder, name));
    }

    fn action_extract(&mut self) {
        let Screen::Archive(view) = &self.screen else { return };
        let keys = view.selected_keys();
        let patterns = if keys.is_empty() {
            if view.inner.is_empty() { Vec::new() } else { vec![view.inner.clone()] }
        } else {
            keys
        };
        let what = match view.selected.len() {
            0 if view.inner.is_empty() => tr("Alles").to_string(),
            0 => trf("Ordner {name}", &[("name", &view.inner)]),
            1 => view.rows[*view.selected.iter().next().unwrap()].name.clone(),
            n => trf("{n} markierte Einträge", &[("n", &n.to_string())]),
        };
        self.dialog = Dialog::Extract(ExtractForm {
            archive: view.path.clone(),
            password: view.password.clone(),
            patterns,
            strip: if view.inner.is_empty() { 0 } else { view.inner.split('/').count() },
            dest: default_dest(&view.path).to_string_lossy().into_owned(),
            overwrite: Overwrite::Skip,
            what,
            open_after: true,
            quit_after: false,
        });
    }

    fn action_test(&mut self) {
        let Screen::Archive(view) = &self.screen else { return };
        let (archive, password) = (view.path.clone(), view.password.clone());
        let label = trf("Prüfe {name}", &[("name", &file_name(&archive))]);
        self.start_job(label, false, move |progress| {
            let result = ops::extract_any(&archive, password.as_deref(), None, &[], Overwrite::Fail, 0, progress).map(|failed| {
                let total = lang::size(progress.total.load(Ordering::Relaxed));
                let mut lines = summary(trf("{size} geprüft.", &[("size", &total)]), failed);
                if lines.len() == 1 {
                    lines.push(tr("Das Archiv ist in Ordnung.").into());
                }
                lines
            });
            let mut outcome = Outcome::from("Prüfung abgeschlossen", result);
            if outcome.ok && outcome.lines.len() > 2 {
                outcome.ok = false;
                outcome.title = tr("Das Archiv hat Fehler").into();
            }
            outcome
        });
    }

    fn action_add(&mut self) {
        if let Some(files) = rfd::FileDialog::new().set_title(tr("Dateien zum Archiv hinzufügen")).pick_files() {
            self.start_add(files);
        }
    }

    fn action_delete(&mut self) {
        let Screen::Archive(view) = &self.screen else { return };
        let keys = view.selected_keys();
        if !keys.is_empty() {
            self.dialog = Dialog::ConfirmDelete(keys);
        }
    }

    fn action_details(&mut self) {
        let Screen::Archive(view) = &self.screen else { return };
        self.dialog = Dialog::Details(DetailsForm { comment: view.listing.comment.clone(), recovery: 5 });
    }

    fn action_repair(&mut self) {
        let Screen::Archive(view) = &self.screen else { return };
        let archive = view.path.clone();
        self.repair(archive);
    }

    fn repair(&mut self, archive: PathBuf) {
        let stem = archive.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let output = archive.with_file_name(format!("{stem}.repariert.qz"));
        let label = trf("Repariere {name}", &[("name", &file_name(&archive))]);
        self.start_job(label, false, move |progress| match ops::repair(&archive, &output, progress) {
            Ok(report) if report.lost_stripes == 0 => {
                let lines = vec![
                    trf("{n} beschädigte Stellen gefunden und repariert.", &[("n", &report.repaired.to_string())]),
                    trf("Repariertes Archiv: {name}", &[("name", &file_name(&output))]),
                ];
                Outcome { reveal: Some(output), ..Outcome::done("Repariert", lines) }
            }
            Ok(_) => Outcome {
                lines: vec![tr("Der Schaden ist größer, als die Schutzdaten ausgleichen können.").into()],
                ..Outcome::failed(String::new())
            },
            Err(e) => Outcome::failed(format!("{e:#}")),
        });
    }

    fn open_row(&mut self, index: usize) {
        let Screen::Archive(view) = &mut self.screen else { return };
        let Some(row) = view.rows.get(index).cloned() else { return };
        if row.is_dir {
            view.enter(row.key);
            return;
        }
        let (archive, password) = (view.path.clone(), view.password.clone());
        let dir = std::env::temp_dir().join("quarz-ansicht").join(std::process::id().to_string());
        let strip = row.key.split('/').count() - 1;
        let label = trf("Öffne {name}", &[("name", &row.name)]);
        self.start_job(label, false, move |progress| {
            let patterns = vec![row.key.clone()];
            match ops::extract_any(&archive, password.as_deref(), Some(&dir), &patterns, Overwrite::Replace, strip, progress) {
                Ok(failed) if failed.is_empty() => {
                    Outcome { title: String::new(), lines: Vec::new(), ok: true, open: Some(dir.join(&row.name)), reveal: None, repair: None }
                }
                Ok(failed) => Outcome::failed(failed.join("\n")),
                Err(e) => Outcome::failed(format!("{e:#}")),
            }
        });
    }

    fn dropped(&mut self, files: Vec<PathBuf>) {
        if files.is_empty() || self.job.is_some() || !matches!(self.dialog, Dialog::None) {
            return;
        }
        if let Screen::Archive(view) = &self.screen {
            if view.listing.editable {
                return self.start_add(files);
            }
        }
        if files.len() == 1 && files[0].is_file() && ops::is_archive(&files[0]) {
            let archive = files[0].clone();
            return self.with_archive(&archive, Pending::Browse);
        }
        self.open_pack_form(files);
    }

    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let dropped: Vec<PathBuf> = ui.ctx().input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        self.dropped(dropped);

        if self.compact {
            if self.job.is_none() && matches!(self.dialog, Dialog::None) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            let frame = egui::Frame::new().fill(colors(ui).bg).inner_margin(egui::Margin::same(22));
            let used = egui::CentralPanel::default()
                .frame(frame)
                .show(ui, |ui| {
                    self.draw_dialog(ui);
                    self.draw_job(ui);
                    ui.min_rect().height()
                })
                .inner;
            let wanted = (used + 46.0).clamp(150.0, 900.0);
            if let Some(inner) = ui.ctx().input(|i| i.viewport().inner_rect) {
                if (inner.height() - wanted).abs() > 4.0 {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(inner.width(), wanted)));
                }
            }
            return;
        }

        match &self.screen {
            Screen::Home => self.draw_home(ui),
            Screen::Archive(_) => self.draw_archive(ui),
        }
        self.draw_dialog(ui);
        self.draw_job(ui);
    }

    fn draw_home(&mut self, ui: &mut egui::Ui) {
        #[derive(Clone)]
        enum Act {
            PackFiles,
            PackFolder,
            OpenArchive,
            Recent(PathBuf),
        }
        let mut act = None;
        let hovering_files = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        egui::CentralPanel::default().show(ui, |ui| {
            let p = colors(ui);
            let width = (ui.available_width() - 48.0).clamp(360.0, 660.0);
            let pad = ((ui.available_width() - width) / 2.0).max(0.0);
            ui.add_space((ui.available_height() * 0.06).clamp(10.0, 60.0));
            ui.horizontal(|ui| {
                ui.add_space(pad);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        ui.label(bold("Quarz").size(30.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let mut chosen = lang::current();
                            egui::ComboBox::from_id_salt("language")
                                .selected_text(format!("{I_GLOBE}  {}", chosen.name()))
                                .show_ui(ui, |ui| {
                                    for language in Lang::ALL {
                                        ui.selectable_value(&mut chosen, language, language.name());
                                    }
                                })
                                .response
                                .on_hover_text(tr("Sprache"));
                            if chosen != lang::current() {
                                lang::save(chosen);
                            }
                        });
                    });
                    muted(ui, tr("Dateien kleiner packen und Archive öffnen"));
                    ui.add_space(14.0);

                    let (zone, _) = ui.allocate_exact_size(egui::vec2(width, 150.0), egui::Sense::hover());
                    let (fill, edge) = if hovering_files { (p.hover, p.accent) } else { (p.card, p.border) };
                    ui.painter().rect_filled(zone, 12, fill);
                    let inset = zone.shrink(1.0);
                    let corners = [inset.left_top(), inset.right_top(), inset.right_bottom(), inset.left_bottom(), inset.left_top()];
                    ui.painter().extend(egui::Shape::dashed_line(&corners, egui::Stroke::new(1.5, edge), 7.0, 5.0));
                    let center = zone.center();
                    ui.painter().text(center - egui::vec2(0.0, 30.0), egui::Align2::CENTER_CENTER, I_EXTRACT, egui::FontId::proportional(30.0), p.accent);
                    ui.painter().text(
                        center + egui::vec2(0.0, 10.0),
                        egui::Align2::CENTER_CENTER,
                        tr("Dateien oder Ordner hierher ziehen"),
                        egui::FontId::new(17.0, FontFamily::Name("bold".into())),
                        p.text,
                    );
                    ui.painter().text(
                        center + egui::vec2(0.0, 36.0),
                        egui::Align2::CENTER_CENTER,
                        tr("zum Packen – oder ein Archiv, um es zu öffnen"),
                        egui::FontId::proportional(13.5),
                        p.muted,
                    );
                    ui.add_space(6.0);

                    let gap = 12.0;
                    let size = egui::vec2((width - 2.0 * gap) / 3.0, 104.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        if tile(ui, I_FILE, tr("Dateien packen"), tr("auswählen und verkleinern"), size).clicked() {
                            act = Some(Act::PackFiles);
                        }
                        if tile(ui, I_FOLDER, tr("Ordner packen"), tr("einen ganzen Ordner"), size).clicked() {
                            act = Some(Act::PackFolder);
                        }
                        if tile(ui, I_OPEN, tr("Archiv öffnen"), tr("ansehen und entpacken"), size).clicked() {
                            act = Some(Act::OpenArchive);
                        }
                    });

                    if !self.recent.is_empty() {
                        ui.add_space(14.0);
                        ui.label(bold(tr("Zuletzt geöffnet")));
                        for path in &self.recent {
                            let text = format!("{I_ARCHIVE}   {}", file_name(path));
                            let folder = path.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                            let button = egui::Button::new(text).frame(false).right_text(RichText::new(folder).color(p.muted).small());
                            if ui.add_sized([width, 30.0], button).clicked() {
                                act = Some(Act::Recent(path.clone()));
                            }
                        }
                    }
                });
            });
        });
        match act {
            Some(Act::PackFiles) => {
                if let Some(files) = rfd::FileDialog::new().set_title(tr("Dateien zum Packen auswählen")).pick_files() {
                    self.open_pack_form(files);
                }
            }
            Some(Act::PackFolder) => {
                if let Some(folder) = rfd::FileDialog::new().set_title(tr("Ordner zum Packen auswählen")).pick_folder() {
                    self.open_pack_form(vec![folder]);
                }
            }
            Some(Act::OpenArchive) => {
                let filter = ["qz", "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "xz", "zst", "jar"];
                let dialog = rfd::FileDialog::new().set_title(tr("Archiv öffnen")).add_filter(tr("Archive"), &filter).add_filter(tr("Alle Dateien"), &["*"]);
                if let Some(file) = dialog.pick_file() {
                    self.with_archive(&file, Pending::Browse);
                }
            }
            Some(Act::Recent(path)) => self.with_archive(&path, Pending::Browse),
            None => {}
        }
    }

    fn draw_archive(&mut self, ui: &mut egui::Ui) {
        #[derive(Clone)]
        enum Act {
            Home,
            Up(usize),
            Extract,
            Add,
            Delete,
            Test,
            Details,
            Repair,
            Reveal,
            Open(usize),
            Click(usize, egui::Modifiers),
            ClearSelection,
        }
        let mut act = None;
        let busy = self.job.is_some();
        let Screen::Archive(view) = &self.screen else { return };
        let p = colors(ui);
        let editable = view.listing.editable;
        let quarz_archive = view.listing.kind == "Quarz";

        let header = egui::Frame::new().fill(p.card).inner_margin(egui::Margin::symmetric(18, 12));
        egui::Panel::top("archive-header").frame(header).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(I_BACK).frame(false)).on_hover_text(tr("Zur Startseite")).clicked() {
                    act = Some(Act::Home);
                }
                ui.label(RichText::new(I_ARCHIVE).size(26.0).color(p.accent));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(bold(file_name(&view.path)).size(18.0));
                    let files = view.listing.entries.iter().filter(|e| !e.is_dir).count();
                    let size: u64 = view.listing.entries.iter().map(|e| e.size).sum();
                    let mut line = vec![
                        trf("{n} Dateien", &[("n", &files.to_string())]),
                        lang::size(size),
                        trf("im Archiv {size}", &[("size", &lang::size(view.listing.packed))]),
                    ];
                    line.extend(facts(&view.listing, false));
                    muted(ui, line.join("  ·  "));
                });
            });
            ui.add_space(6.0);
            ui.add_enabled_ui(!busy, |ui| {
                ui.horizontal(|ui| {
                    let label = match view.selected.len() {
                        0 => format!("{I_EXTRACT}  {}", tr("Alles entpacken")),
                        n => format!("{I_EXTRACT}  {}", trf("Auswahl entpacken ({n})", &[("n", &n.to_string())])),
                    };
                    if primary_button(ui, &label).clicked() {
                        act = Some(Act::Extract);
                    }
                    if editable {
                        if ui.button(format!("{I_ADD}  {}", tr("Hinzufügen"))).clicked() {
                            act = Some(Act::Add);
                        }
                        let delete = ui.add_enabled(!view.selected.is_empty(), egui::Button::new(format!("{I_DELETE}  {}", tr("Löschen"))));
                        if delete.on_disabled_hover_text(tr("Erst Einträge markieren")).clicked() {
                            act = Some(Act::Delete);
                        }
                    }
                    if ui.button(format!("{I_CHECK}  {}", tr("Prüfen"))).on_hover_text(tr("Kontrolliert, ob das Archiv unbeschädigt ist")).clicked() {
                        act = Some(Act::Test);
                    }
                    ui.menu_button(format!("{I_MORE}  {}", tr("Mehr")), |ui| {
                        if quarz_archive && ui.button(tr("Eigenschaften und Schutz …")).clicked() {
                            act = Some(Act::Details);
                            ui.close();
                        }
                        if quarz_archive && ui.button(tr("Beschädigtes Archiv reparieren")).clicked() {
                            act = Some(Act::Repair);
                            ui.close();
                        }
                        if ui.button(tr("Im Explorer anzeigen")).clicked() {
                            act = Some(Act::Reveal);
                            ui.close();
                        }
                    });
                });
            });
        });

        let crumbs = egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(18, 6));
        egui::Panel::top("archive-path").frame(crumbs).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let parts: Vec<&str> = view.inner.split('/').filter(|s| !s.is_empty()).collect();
                if ui.add(egui::Button::new(file_name(&view.path)).frame(false)).clicked() {
                    act = Some(Act::Up(0));
                }
                for (depth, part) in parts.iter().enumerate() {
                    ui.label(RichText::new(I_CHEVRON).size(10.0).color(p.muted));
                    if ui.add(egui::Button::new(*part).frame(false)).clicked() {
                        act = Some(Act::Up(depth + 1));
                    }
                }
            });
        });

        let footer = egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(18, 6));
        egui::Panel::bottom("archive-hint").frame(footer).show(ui, |ui| {
            let hint = if !view.selected.is_empty() {
                let size: u64 = view.selected.iter().filter_map(|&i| view.rows.get(i)).map(|r| r.size).sum();
                format!("{}  ·  {}", trf("{n} markiert", &[("n", &view.selected.len().to_string())]), lang::size(size))
            } else if editable {
                tr("Doppelklick öffnet eine Datei. Dateien hierher ziehen, um sie hinzuzufügen.").to_string()
            } else {
                tr("Doppelklick öffnet eine Datei.").to_string()
            };
            muted(ui, hint);
        });

        let body = egui::Frame::new().fill(p.card).inner_margin(egui::Margin::symmetric(10, 4));
        egui::CentralPanel::default().frame(body).show(ui, |ui| {
            let modifiers = ui.input(|i| i.modifiers);
            if view.rows.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| muted(ui, tr("Dieser Ordner ist leer.")));
                return;
            }
            TableBuilder::new(ui)
                .sense(egui::Sense::click())
                .resizable(false)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::remainder().at_least(240.0).clip(true))
                .column(Column::exact(110.0))
                .column(Column::exact(150.0))
                .header(28.0, |mut header| {
                    for title in [tr("Name"), tr("Größe"), tr("Geändert")] {
                        header.col(|ui| {
                            ui.add_space(6.0);
                            ui.label(RichText::new(title).color(p.muted).small());
                        });
                    }
                })
                .body(|body| {
                    body.rows(30.0, view.rows.len(), |mut row| {
                        let index = row.index();
                        let item = &view.rows[index];
                        row.set_selected(view.selected.contains(&index));
                        row.col(|ui| {
                            ui.add_space(6.0);
                            let (icon, tint) = if item.is_dir { (I_FOLDER, p.folder) } else { (I_FILE, p.muted) };
                            ui.add(egui::Label::new(RichText::new(icon).size(17.0).color(tint)).wrap_mode(egui::TextWrapMode::Extend));
                            ui.label(&item.name);
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(lang::size(item.size)).color(p.muted));
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(lang::time(item.mtime)).color(p.muted));
                        });
                        let response = row.response();
                        if response.double_clicked() {
                            act = Some(Act::Open(index));
                        } else if response.clicked() {
                            act = Some(Act::Click(index, modifiers));
                        }
                    });
                });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                act = Some(Act::ClearSelection);
            }
        });

        let Some(act) = act else { return };
        match act {
            Act::Home => self.screen = Screen::Home,
            Act::Extract => self.action_extract(),
            Act::Add => self.action_add(),
            Act::Delete => self.action_delete(),
            Act::Test => self.action_test(),
            Act::Details => self.action_details(),
            Act::Repair => self.action_repair(),
            Act::Open(index) if !busy => self.open_row(index),
            Act::Open(_) => {}
            Act::Reveal | Act::Up(_) | Act::Click(..) | Act::ClearSelection => {
                let Screen::Archive(view) = &mut self.screen else { return };
                match act {
                    Act::Reveal => reveal(&view.path),
                    Act::Up(depth) => {
                        let inner: Vec<&str> = view.inner.split('/').filter(|s| !s.is_empty()).take(depth).collect();
                        let inner = inner.join("/");
                        view.enter(inner);
                    }
                    Act::Click(index, modifiers) => view.click(index, modifiers),
                    _ => view.selected.clear(),
                }
            }
        }
    }

    fn draw_job(&mut self, ui: &mut egui::Ui) {
        let compact = self.compact;
        let Some(job) = &mut self.job else { return };
        if job.handle.as_ref().is_some_and(|h| h.is_finished()) {
            let outcome = job.handle.take().unwrap().join().unwrap_or_else(|_| Outcome {
                lines: vec![tr("Der Vorgang ist abgestürzt.").into()],
                ..Outcome::failed(String::new())
            });
            let quit = job.quit_after && outcome.ok;
            self.job = None;
            let mut outcome = outcome;
            if let (false, None, Screen::Archive(view)) = (outcome.ok, &outcome.repair, &self.screen) {
                if !outcome.lines.is_empty() && ops::check_recovery(&view.path, &Progress::default()).ok().flatten().is_some_and(|r| r.damaged > 0) {
                    outcome.lines.push(tr("Das Archiv enthält Schutzdaten und lässt sich wahrscheinlich reparieren.").into());
                    outcome.repair = Some(view.path.clone());
                }
            }
            self.refresh_archive();
            if let Some(file) = &outcome.open {
                open_with_default(file);
            }
            if quit {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            } else if !outcome.title.is_empty() {
                self.dialog = Dialog::Result(outcome);
            }
            ui.ctx().request_repaint();
            return;
        }
        ui.ctx().request_repaint_after(Duration::from_millis(100));
        job.shown = job.shown.max(job.progress.fraction());
        let fraction = job.shown;
        let label = job.label.clone();
        let cancelling = job.progress.cancel.load(Ordering::Relaxed);
        let (cancel, _) = present(compact, ui, "job", 420.0, |ui| {
            ui.label(bold(label).size(17.0));
            ui.add_space(4.0);
            progress_bar(ui, fraction);
            ui.horizontal(|ui| {
                muted(ui, if cancelling { tr("Wird abgebrochen …").to_string() } else { format!("{:.0} %", fraction * 100.0) });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled(!cancelling, egui::Button::new(tr("Abbrechen"))).clicked()
                })
                .inner
            })
            .inner
        });
        if cancel {
            if let Some(job) = &self.job {
                job.progress.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    fn refresh_archive(&mut self) {
        let Screen::Archive(view) = &mut self.screen else { return };
        if let Ok(listing) = ops::listing(&view.path, view.password.as_deref()) {
            view.listing = Arc::new(listing);
            view.refresh();
        }
    }

    fn draw_dialog(&mut self, ui: &mut egui::Ui) {
        let dialog = std::mem::replace(&mut self.dialog, Dialog::None);
        let compact = self.compact;
        match dialog {
            Dialog::None => {}
            Dialog::Result(outcome) => {
                let mut repair = false;
                let ((close, show), dismissed) = present(compact, ui, "result", 440.0, |ui| {
                    let p = colors(ui);
                    ui.horizontal(|ui| {
                        let (icon, tint) = if outcome.ok { (I_CHECK, p.good) } else { (I_WARNING, p.danger) };
                        ui.label(RichText::new(icon).size(22.0).color(tint));
                        ui.label(bold(outcome.title.clone()).size(18.0));
                    });
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        for line in &outcome.lines {
                            ui.label(line);
                        }
                    });
                    let mut show = false;
                    let mut close = false;
                    ui.add_space(6.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close = primary_button(ui, tr("Schließen")).clicked();
                        if outcome.reveal.is_some() && ui.button(format!("{I_FOLDER}  {}", tr("Im Explorer zeigen"))).clicked() {
                            show = true;
                        }
                        if outcome.repair.is_some() && ui.button(format!("{I_SHIELD}  {}", tr("Reparieren"))).clicked() {
                            repair = true;
                        }
                    });
                    (close, show)
                });
                if show {
                    if let Some(path) = &outcome.reveal {
                        reveal(path);
                    }
                }
                if repair {
                    if let Some(archive) = outcome.repair.clone() {
                        self.repair(archive);
                    }
                } else if !(close || show || dismissed) {
                    self.dialog = Dialog::Result(outcome);
                }
            }
            Dialog::Password(mut form) => {
                let (choice, dismissed) = present(compact, ui, "password", 380.0, |ui| {
                    let p = colors(ui);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(I_LOCK).size(20.0).color(p.accent));
                        ui.label(bold(tr("Passwort eingeben")).size(18.0));
                    });
                    muted(ui, trf("{name} ist geschützt.", &[("name", &file_name(&form.archive))]));
                    if form.wrong {
                        ui.colored_label(p.danger, tr("Das Passwort stimmt nicht. Bitte noch einmal versuchen."));
                    }
                    let field = ui.add(egui::TextEdit::singleline(&mut form.text).password(true).desired_width(f32::INFINITY).hint_text(tr("Passwort")));
                    field.request_focus();
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    match confirm_row(ui, tr("Öffnen"), !form.text.is_empty()) {
                        Choice::Stay if enter && !form.text.is_empty() => Choice::Confirm,
                        other => other,
                    }
                });
                if choice == Choice::Confirm {
                    self.passwords.insert(form.archive.clone(), form.text.clone());
                    self.with_archive(&form.archive, form.then);
                } else if !(choice == Choice::Cancel || dismissed) {
                    self.dialog = Dialog::Password(form);
                }
            }
            Dialog::ConfirmDelete(patterns) => {
                let (choice, dismissed) = present(compact, ui, "delete", 400.0, |ui| {
                    ui.label(bold(tr("Aus dem Archiv löschen?")).size(18.0));
                    ui.label(match patterns.len() {
                        1 => trf("„{name}“ wird aus dem Archiv entfernt.", &[("name", patterns[0].rsplit('/').next().unwrap_or(""))]),
                        n => trf("{n} markierte Einträge werden aus dem Archiv entfernt.", &[("n", &n.to_string())]),
                    });
                    muted(ui, tr("Die Dateien auf deinem Computer bleiben unberührt."));
                    confirm_row(ui, tr("Löschen"), true)
                });
                if choice == Choice::Confirm {
                    let Screen::Archive(view) = &self.screen else { return };
                    let (archive, password) = (view.path.clone(), view.password.clone());
                    self.start_job(tr("Lösche aus dem Archiv").into(), false, move |progress| {
                        let result = ops::delete(&archive, &patterns, password.as_deref(), progress)
                            .map(|count| vec![trf("{n} Dateien entfernt.", &[("n", &count.to_string())])]);
                        Outcome::from("Gelöscht", result)
                    });
                } else if !(choice == Choice::Cancel || dismissed) {
                    self.dialog = Dialog::ConfirmDelete(patterns);
                }
            }
            Dialog::Extract(mut form) => {
                let (choice, dismissed) = present(compact, ui, "extract", 500.0, |ui| {
                    ui.label(bold(tr("Entpacken")).size(18.0));
                    muted(ui, format!("{}  ·  {}", file_name(&form.archive), form.what));
                    ui.add_space(4.0);
                    ui.label(tr("Wohin?"));
                    ui.horizontal(|ui| {
                        let room = ui.available_width() - 110.0;
                        ui.add(egui::TextEdit::singleline(&mut form.dest).desired_width(room));
                        if ui.button(tr("Ändern …")).clicked() {
                            if let Some(dir) = rfd::FileDialog::new().set_title(tr("Zielordner")).pick_folder() {
                                form.dest = dir.to_string_lossy().into_owned();
                            }
                        }
                    });
                    ui.checkbox(&mut form.open_after, tr("Ordner danach im Explorer öffnen"));
                    let mut replace = form.overwrite == Overwrite::Replace;
                    ui.checkbox(&mut replace, tr("Vorhandene Dateien überschreiben"));
                    form.overwrite = if replace { Overwrite::Replace } else { Overwrite::Skip };
                    confirm_row(ui, tr("Entpacken"), !form.dest.trim().is_empty())
                });
                if choice == Choice::Confirm {
                    self.start_extract(form);
                } else if !(choice == Choice::Cancel || dismissed) {
                    self.dialog = Dialog::Extract(form);
                }
            }
            Dialog::Pack(mut form) => {
                let (choice, dismissed) = present(compact, ui, "pack", 520.0, |ui| form.draw(ui));
                if choice == Choice::Confirm {
                    if form.folder.join(form.name.trim()).exists() {
                        let text = trf("„{name}“ gibt es schon. Bitte einen anderen Namen wählen.", &[("name", form.name.trim())]);
                        self.dialog = Dialog::Result(Outcome { lines: vec![text], ..Outcome::failed(String::new()) });
                    } else {
                        self.start_create(form, compact);
                    }
                } else if !(choice == Choice::Cancel || dismissed) {
                    self.dialog = Dialog::Pack(form);
                }
            }
            Dialog::Details(mut form) => {
                #[derive(PartialEq)]
                enum Pick {
                    Stay,
                    Close,
                    Comment,
                    Lock,
                    Protect,
                    Sfx,
                    Join,
                }
                let Screen::Archive(view) = &self.screen else { return };
                let (archive, password, listing) = (view.path.clone(), view.password.clone(), view.listing.clone());
                let split = crate::volume::parts_of(&archive).is_some();
                let (mut pick, dismissed) = present(compact, ui, "details", 500.0, |ui| {
                    let mut pick = Pick::Stay;
                    ui.label(bold(tr("Eigenschaften und Schutz")).size(18.0));
                    muted(ui, facts(&listing, true).join("  ·  "));
                    ui.add_enabled_ui(listing.editable, |ui| {
                        ui.add_space(4.0);
                        ui.label(tr("Kommentar"));
                        ui.horizontal(|ui| {
                            let room = ui.available_width() - 120.0;
                            ui.add(egui::TextEdit::singleline(&mut form.comment).desired_width(room));
                            if ui.button(tr("Speichern")).clicked() {
                                pick = Pick::Comment;
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(format!("{I_SHIELD}  {}", tr("Schutz gegen Beschädigung")));
                        muted(ui, tr("Macht das Archiv etwas größer, dafür lässt es sich bei Schäden reparieren."));
                        ui.horizontal(|ui| {
                            let choices = [(5u8, tr("Normal"), tr("5 % größer")), (10, tr("Stark"), tr("10 % größer"))];
                            segmented(ui, &mut form.recovery, &choices, 78.0);
                            if ui.button(tr("Schutz hinzufügen")).clicked() {
                                pick = Pick::Protect;
                            }
                        });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button(tr("Als selbstentpackende .exe speichern")).clicked() {
                                pick = Pick::Sfx;
                            }
                            if ui.button(format!("{I_LOCK}  {}", tr("Sperren"))).on_hover_text(tr("Danach kann das Archiv nicht mehr geändert werden")).clicked() {
                                pick = Pick::Lock;
                            }
                        });
                    });
                    if split && ui.button(tr("Teile zu einer Datei zusammenfügen")).clicked() {
                        pick = Pick::Join;
                    }
                    ui.add_space(6.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary_button(ui, tr("Schließen")).clicked() {
                            pick = Pick::Close;
                        }
                    });
                    pick
                });
                if dismissed && pick == Pick::Stay {
                    pick = Pick::Close;
                }
                match pick {
                    Pick::Stay => self.dialog = Dialog::Details(form),
                    Pick::Close => {}
                    Pick::Comment => {
                        let comment = form.comment.clone();
                        self.start_job(tr("Speichere Kommentar").into(), false, move |progress| {
                            let result = ops::set_comment(&archive, &comment, password.as_deref(), progress).map(|()| vec![tr("Der Kommentar ist gespeichert.").to_string()]);
                            Outcome::from("Gespeichert", result)
                        });
                    }
                    Pick::Lock => self.start_job(tr("Sperre Archiv").into(), false, move |progress| {
                        let result = ops::lock(&archive, password.as_deref(), progress).map(|()| vec![tr("Das Archiv kann jetzt nicht mehr geändert werden.").to_string()]);
                        Outcome::from("Gesperrt", result)
                    }),
                    Pick::Protect => {
                        let percent = form.recovery;
                        self.start_job(tr("Berechne Schutzdaten").into(), false, move |progress| {
                            let result = ops::protect(&archive, percent, progress).map(|()| {
                                vec![trf("{p} % Schutzdaten hinzugefügt. Kleinere Schäden lassen sich jetzt reparieren.", &[("p", &percent.to_string())])]
                            });
                            Outcome::from("Geschützt", result)
                        });
                    }
                    Pick::Sfx => self.start_job(tr("Erzeuge selbstentpackende Datei").into(), false, move |_progress| {
                        let output = archive.with_extension("exe");
                        let result = std::env::current_exe().map_err(anyhow::Error::from).and_then(|stub| ops::make_sfx(&archive, &stub, &output));
                        match result {
                            Ok(()) => {
                                let lines = vec![trf("Angelegt: {name}", &[("name", &file_name(&output))])];
                                Outcome { reveal: Some(output), ..Outcome::done("Fertig", lines) }
                            }
                            Err(e) => Outcome::failed(format!("{e:#}")),
                        }
                    }),
                    Pick::Join => self.start_job(tr("Füge Teile zusammen").into(), false, move |_progress| {
                        let output = crate::volume::parts_of(&archive).map(|(base, _)| base).unwrap_or_else(|| archive.with_extension("ganz.qz"));
                        match ops::join(&archive, &output) {
                            Ok(count) => {
                                let values = [("n", count.to_string()), ("name", file_name(&output))];
                                let lines = vec![trf("{n} Teile wurden zu {name} zusammengefügt.", &[("n", &values[0].1), ("name", &values[1].1)])];
                                Outcome { reveal: Some(output), ..Outcome::done("Zusammengefügt", lines) }
                            }
                            Err(e) => Outcome::failed(format!("{e:#}")),
                        }
                    }),
                }
            }
        }
    }
}

impl View {
    fn refresh(&mut self) {
        self.rows = archive_rows(&self.listing, &self.inner);
        self.selected.clear();
        self.anchor = None;
    }

    fn enter(&mut self, inner: String) {
        self.inner = inner;
        self.refresh();
    }

    fn selected_keys(&self) -> Vec<String> {
        self.selected.iter().filter_map(|&i| self.rows.get(i)).map(|r| r.key.clone()).collect()
    }

    fn click(&mut self, index: usize, modifiers: egui::Modifiers) {
        if modifiers.shift {
            if let Some(anchor) = self.anchor {
                self.selected = (anchor.min(index)..=anchor.max(index)).collect();
                return;
            }
        }
        if modifiers.command {
            if !self.selected.remove(&index) {
                self.selected.insert(index);
            }
        } else {
            self.selected.clear();
            self.selected.insert(index);
        }
        self.anchor = Some(index);
    }
}

impl PackForm {
    fn new(inputs: Vec<PathBuf>, folder: PathBuf, name: String) -> PackForm {
        PackForm {
            inputs,
            name,
            folder,
            level: 3,
            password: String::new(),
            show_password: false,
            more: false,
            split: String::new(),
            split_gb: false,
            recovery: 0,
            sfx: false,
            comment: String::new(),
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui) -> Choice {
        let p = colors(ui);
        ui.label(bold(tr("Packen")).size(18.0));
        muted(ui, describe_inputs(&self.inputs));
        ui.add_space(4.0);

        ui.label(tr("Name des Archivs"));
        ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            muted(ui, trf("in {folder}", &[("folder", &self.folder.to_string_lossy())]));
            if ui.add(egui::Button::new(RichText::new(tr("Ändern …")).color(p.accent)).frame(false)).clicked() {
                if let Some(dir) = rfd::FileDialog::new().set_title(tr("Wo soll das Archiv gespeichert werden?")).pick_folder() {
                    self.folder = dir;
                }
            }
        });

        ui.add_space(4.0);
        ui.label(tr("Wie stark verkleinern?"));
        let levels = [
            (2u8, tr("Schnell"), tr("Geht am schnellsten, wird etwas größer")),
            (3, tr("Normal"), tr("Guter Mittelweg – empfohlen")),
            (4, tr("Klein"), tr("Dauert deutlich länger, wird am kleinsten")),
        ];
        segmented(ui, &mut self.level, &levels, 104.0);

        ui.add_space(4.0);
        ui.label(tr("Passwort (wenn gewünscht)"));
        ui.horizontal(|ui| {
            let room = ui.available_width() - 130.0;
            ui.add(egui::TextEdit::singleline(&mut self.password).password(!self.show_password).desired_width(room).hint_text(tr("leer lassen für kein Passwort")));
            ui.checkbox(&mut self.show_password, tr("anzeigen"));
        });

        ui.add_space(2.0);
        let arrow = if self.more { "\u{E70E}" } else { "\u{E70D}" };
        if ui.add(egui::Button::new(RichText::new(format!("{arrow}  {}", tr("Weitere Optionen"))).color(p.accent)).frame(false)).clicked() {
            self.more = !self.more;
        }
        if self.more {
            egui::Grid::new("pack-more").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
                ui.label(tr("In Teile aufteilen"));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.split).desired_width(70.0).hint_text(tr("nein")));
                    ui.selectable_value(&mut self.split_gb, false, "MB");
                    ui.selectable_value(&mut self.split_gb, true, "GB");
                    muted(ui, tr("pro Teil"));
                });
                ui.end_row();

                ui.label(tr("Schutz gegen Beschädigung"));
                let choices = [
                    (0u8, tr("Aus"), ""),
                    (5, tr("Normal"), tr("Das Archiv wird 5 % größer und lässt sich bei kleinen Schäden reparieren")),
                    (10, tr("Stark"), tr("Das Archiv wird 10 % größer und verträgt mehr Schaden")),
                ];
                segmented(ui, &mut self.recovery, &choices, 78.0);
                ui.end_row();

                ui.label(tr("Selbstentpackend"));
                ui.checkbox(&mut self.sfx, tr("als .exe speichern, die sich selbst entpackt"));
                ui.end_row();

                ui.label(tr("Kommentar"));
                ui.add(egui::TextEdit::singleline(&mut self.comment).desired_width(300.0));
                ui.end_row();

                ui.label(tr("Genaue Stufe"));
                egui::ComboBox::from_id_salt("level").selected_text(lang::level_name(self.level)).show_ui(ui, |ui| {
                    for level in crate::codec::LEVELS {
                        ui.selectable_value(&mut self.level, level.id, lang::level_name(level.id));
                    }
                });
                ui.end_row();
            });
        }

        let wanted = if self.sfx { "exe" } else { "qz" };
        let current = PathBuf::from(&self.name);
        let extension = current.extension().map(|e| e.to_string_lossy().to_lowercase());
        if matches!(extension.as_deref(), Some("exe") | Some("qz")) && extension.as_deref() != Some(wanted) {
            self.name = current.with_extension(wanted).to_string_lossy().into_owned();
        }
        confirm_row(ui, tr("Packen"), !self.name.trim().is_empty())
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}
