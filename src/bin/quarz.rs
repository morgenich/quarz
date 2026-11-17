#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use quarz::gui::{self, App, Start};
use quarz::lang::tr;
use quarz::ops;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(windows)]
mod shell {
    use std::path::Path;
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    const FOREIGN: &[&str] = &[".zip", ".7z", ".rar", ".tar", ".gz", ".tgz", ".bz2", ".xz", ".zst"];

    fn verb(root: &RegKey, key: &str, label: &str, command: &str, icon: &str) -> std::io::Result<()> {
        let (item, _) = root.create_subkey(key)?;
        item.set_value("MUIVerb", &label)?;
        item.set_value("Icon", &icon)?;
        let (cmd, _) = root.create_subkey(format!("{key}\\command"))?;
        cmd.set_value("", &command)
    }

    fn classic_keys() -> Vec<String> {
        let mut keys = vec!["Quarz.Archiv\\shell\\QuarzHier".to_string(), "Quarz.Archiv\\shell\\QuarzOrdner".to_string()];
        for target in ["*", "Directory"] {
            keys.push(format!("{target}\\shell\\QuarzPacken"));
            keys.push(format!("{target}\\shell\\QuarzPackenOptionen"));
        }
        for ext in FOREIGN {
            keys.push(format!("SystemFileAssociations\\{ext}\\shell\\QuarzOeffnen"));
            keys.push(format!("SystemFileAssociations\\{ext}\\shell\\QuarzOrdner"));
        }
        keys
    }

    pub fn install(exe: &Path, classic_entries: bool) -> std::io::Result<()> {
        let exe = exe.to_string_lossy();
        let classes = RegKey::predef(HKEY_CURRENT_USER).create_subkey("Software\\Classes")?.0;
        classes.create_subkey(".qz")?.0.set_value("", &"Quarz.Archiv")?;
        let (kind, _) = classes.create_subkey("Quarz.Archiv")?;
        kind.set_value("", &"Quarz-Archiv")?;
        classes.create_subkey("Quarz.Archiv\\DefaultIcon")?.0.set_value("", &format!("\"{exe}\",0"))?;
        classes.create_subkey("Quarz.Archiv\\shell\\open\\command")?.0.set_value("", &format!("\"{exe}\" \"%1\""))?;
        for key in classic_keys() {
            let _ = classes.delete_subkey_all(key);
        }
        let _ = classes.delete_subkey_all("*\\shell\\QuarzPacken");
        if !classic_entries {
            return Ok(());
        }
        let here = format!("\"{exe}\" --extract-here \"%1\"");
        let to = format!("\"{exe}\" --extract-to \"%1\"");
        verb(&classes, "Quarz.Archiv\\shell\\QuarzHier", "Hier entpacken", &here, &exe)?;
        verb(&classes, "Quarz.Archiv\\shell\\QuarzOrdner", "In eigenen Ordner entpacken", &to, &exe)?;
        let now = format!("\"{exe}\" --pack-now \"%1\"");
        let add = format!("\"{exe}\" --add \"%1\"");
        for target in ["*", "Directory"] {
            verb(&classes, &format!("{target}\\shell\\QuarzPacken"), "Mit Quarz packen", &now, &exe)?;
            verb(&classes, &format!("{target}\\shell\\QuarzPackenOptionen"), "Mit Quarz packen (Optionen …)", &add, &exe)?;
        }
        for ext in FOREIGN {
            let base = format!("SystemFileAssociations\\{ext}\\shell");
            verb(&classes, &format!("{base}\\QuarzOeffnen"), "Mit Quarz öffnen", &format!("\"{exe}\" \"%1\""), &exe)?;
            verb(&classes, &format!("{base}\\QuarzOrdner"), "Mit Quarz in eigenen Ordner entpacken", &to, &exe)?;
        }
        Ok(())
    }

    pub fn uninstall() -> std::io::Result<()> {
        let classes = RegKey::predef(HKEY_CURRENT_USER).create_subkey("Software\\Classes")?.0;
        for key in classic_keys() {
            let _ = classes.delete_subkey_all(key);
        }
        let _ = classes.delete_subkey_all(".qz");
        let _ = classes.delete_subkey_all("Quarz.Archiv");
        Ok(())
    }

    pub fn classic_menu(on: bool) -> std::io::Result<()> {
        let classes = RegKey::predef(HKEY_CURRENT_USER).create_subkey("Software\\Classes")?.0;
        let key = "CLSID\\{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}";
        if on {
            classes.create_subkey(format!("{key}\\InprocServer32"))?.0.set_value("", &"")
        } else {
            let _ = classes.delete_subkey_all(key);
            Ok(())
        }
    }

    const PACKAGE: &str = "Quarz.Kontextmenue";

    fn powershell(script: &str) -> (bool, String) {
        use std::os::windows::process::CommandExt;
        let output = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
            .creation_flags(0x0800_0000)
            .output();
        match output {
            Ok(out) => {
                let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
                (out.status.success(), text.trim().to_string())
            }
            Err(e) => (false, e.to_string()),
        }
    }

    fn quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "''"))
    }

    pub fn modern_registered() -> bool {
        let (ok, text) = powershell(&format!("(Get-AppxPackage -Name {PACKAGE} | Measure-Object).Count"));
        ok && text.trim() != "0" && !text.trim().is_empty()
    }

    pub fn modern_files(dir: &Path) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let (package, certificate) = (dir.join("quarz-menu.msix"), dir.join("quarz-menu.cer"));
        (package.is_file() && certificate.is_file() && dir.join("quarz_shell.dll").is_file()).then_some((package, certificate))
    }

    fn register(dir: &Path, package: &Path) -> (bool, String) {
        powershell(&format!(
            "$ErrorActionPreference='Stop'; try {{ Add-AppxPackage -Path {} -ExternalLocation {}; 'ok' }} catch {{ $_.Exception.Message; exit 1 }}",
            quote(package),
            quote(dir)
        ))
    }

    pub fn install_modern(dir: &Path) -> Result<(), String> {
        use quarz::lang::tr;
        let (package, certificate) = modern_files(dir).ok_or(tr("Die Dateien für das Windows-11-Menü fehlen."))?;
        let (ok, first) = register(dir, &package);
        if ok {
            return Ok(());
        }
        if !first.contains("0x800B0109") {
            return Err(first);
        }
        let (approved, text) = powershell(&format!(
            "try {{ $p = Start-Process certutil.exe -ArgumentList '-addstore','TrustedPeople','\"{}\"' -Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode }} catch {{ $_.Exception.Message; exit 1 }}",
            certificate.to_string_lossy()
        ));
        if !approved {
            return Err(if text.is_empty() { tr("Die Bestätigung wurde abgelehnt.").into() } else { text });
        }
        let (ok, second) = register(dir, &package);
        if ok { Ok(()) } else { Err(second) }
    }

    pub fn uninstall_modern(dir: &Path) {
        powershell(&format!("Get-AppxPackage -Name {PACKAGE} | Remove-AppxPackage"));
        if let Some((_, certificate)) = modern_files(dir) {
            powershell(&format!(
                "try {{ $t = (Get-PfxCertificate -FilePath {}).Thumbprint; if (Test-Path \"Cert:\\LocalMachine\\TrustedPeople\\$t\") {{ Start-Process certutil.exe -ArgumentList '-delstore','TrustedPeople',$t -Verb RunAs -WindowStyle Hidden -Wait }} }} catch {{ }}",
                quote(&certificate)
            ));
        }
    }
}

fn collect_paths(mine: Vec<PathBuf>, tag: &str) -> Option<Vec<PathBuf>> {
    use std::io::Write;
    let dir = std::env::temp_dir();
    let (list, lock) = (dir.join(format!("quarz-{tag}.list")), dir.join(format!("quarz-{tag}.lock")));
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&list) {
        for path in &mine {
            let _ = writeln!(file, "{}", path.display());
        }
    }
    let fresh = |path: &Path| {
        std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < Duration::from_secs(5))
    };
    if std::fs::OpenOptions::new().write(true).create_new(true).open(&lock).is_err() {
        if fresh(&lock) {
            return None;
        }
        let _ = std::fs::remove_file(&lock);
        let _ = std::fs::OpenOptions::new().write(true).create_new(true).open(&lock);
    }
    let mut last = 0u64;
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(150));
        let size = std::fs::metadata(&list).map(|m| m.len()).unwrap_or(0);
        if size == last {
            break;
        }
        last = size;
    }
    let text = std::fs::read_to_string(&list).unwrap_or_default();
    let _ = std::fs::remove_file(&list);
    let _ = std::fs::remove_file(&lock);
    let mut paths: Vec<PathBuf> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let path = PathBuf::from(line);
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    Some(if paths.is_empty() { mine } else { paths })
}

fn message(title: &str, text: &str) {
    rfd::MessageDialog::new().set_title(title).set_description(text).show();
}

#[cfg(windows)]
fn install(quiet: bool) {
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut modern = shell::modern_registered();
    if quiet {
        let _ = shell::install(&exe, !modern);
        return;
    }
    let mut note = String::new();
    if !modern && shell::modern_files(&dir).is_some() {
        let question = tr("Soll Quarz direkt im ersten Rechtsklick-Menü von Windows 11 erscheinen?\n\nWindows lässt dort nur signierte App-Pakete zu. Dafür wird das Zertifikat, mit dem das Quarz-Paket signiert ist, auf diesem Computer als vertrauenswürdig eingetragen. Windows fragt gleich nach Administrator-Bestätigung.\n\nBei „Nein“ stehen die Quarz-Einträge unter „Weitere Optionen anzeigen“.");
        let answer = rfd::MessageDialog::new()
            .set_title(tr("Quarz einrichten"))
            .set_description(question)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show();
        if answer == rfd::MessageDialogResult::Yes {
            match shell::install_modern(&dir) {
                Ok(()) => modern = true,
                Err(e) => note = format!("\n\n{}\n{e}", tr("Das Windows-11-Menü konnte nicht eingerichtet werden:")),
            }
        }
    }
    match shell::install(&exe, !modern) {
        Ok(()) if modern => message(
            "Quarz",
            tr("Fertig. Rechtsklick auf Dateien, Ordner oder Archive zeigt jetzt direkt den Eintrag „Quarz“, und .qz-Dateien öffnen sich per Doppelklick."),
        ),
        Ok(()) => message(
            "Quarz",
            &format!(
                "{}{note}",
                tr("Fertig. Die Quarz-Einträge stehen im Rechtsklick-Menü unter „Weitere Optionen anzeigen“, und .qz-Dateien öffnen sich per Doppelklick.")
            ),
        ),
        Err(e) => message("Quarz", &format!("{} {e}", tr("Eintragen fehlgeschlagen:"))),
    }
}

fn smoke(args: &[String]) -> i32 {
    unsafe { std::env::set_var("QUARZ_NO_RECENT", "1") };
    let report = PathBuf::from(args.first().cloned().unwrap_or_else(|| "smoke.txt".into()));
    let result = std::panic::catch_unwind(|| {
        let mut lines = Vec::new();
        let frames = |app: &mut App, what: &str, lines: &mut Vec<String>| {
            egui::__run_test_ui(|ui| app.draw(ui));
            lines.push(format!("{what}: ok"));
        };
        let mut app = App::new(Start::Home);
        frames(&mut app, "home", &mut lines);
        app.preview_pack(vec![PathBuf::from("C:\\Windows\\notepad.exe")], true);
        frames(&mut app, "pack dialog", &mut lines);
        for archive in args.iter().skip(1) {
            let mut app = App::new(Start::Open(PathBuf::from(archive)));
            let opened = app.has_archive();
            frames(&mut app, &format!("archive {archive} (open: {opened})"), &mut lines);
            app.preview_select(&[0]);
            app.preview_extract(None);
            frames(&mut app, "extract dialog", &mut lines);
            app.preview_details();
            frames(&mut app, "details dialog", &mut lines);
            app.preview_result(App::preview_outcome("Test", &["Zeile"], true, None));
            frames(&mut app, "result dialog", &mut lines);
            app.preview_result(App::preview_outcome("", &[], true, None));
            let mut app = App::new(Start::Open(PathBuf::from(archive)));
            app.start_test();
            while app.busy() {
                egui::__run_test_ui(|ui| app.draw(ui));
                std::thread::sleep(Duration::from_millis(20));
            }
            egui::__run_test_ui(|ui| app.draw(ui));
            if let Some(outcome) = app.last_result() {
                lines.push(format!("test: {} / {}", outcome.title, outcome.lines.join(" | ")));
            }
        }
        let mut app = App::new(Start::Home);
        app.preview_password(PathBuf::from("geheim.qz"), true);
        frames(&mut app, "password dialog", &mut lines);
        app.preview_job("Packe", 0.4);
        frames(&mut app, "job", &mut lines);

        let until_idle = |app: &mut App| {
            loop {
                egui::__run_test_ui(|ui| app.draw(ui));
                if !app.busy() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            egui::__run_test_ui(|ui| app.draw(ui));
        };
        let dir = report.with_extension("ordner");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Bilder")).unwrap();
        let content = "Quarz Rechtsklick-Test\n".repeat(500);
        std::fs::write(dir.join("Bilder").join("notiz.txt"), &content).unwrap();
        let mut app = App::new(Start::PackNow(vec![dir.join("Bilder")]));
        until_idle(&mut app);
        let archive = dir.join("Bilder.qz");
        lines.push(format!("pack now: archive written {}, error shown {}", archive.is_file(), app.last_result().is_some()));
        std::fs::remove_dir_all(dir.join("Bilder")).unwrap();
        let mut app = App::new(Start::ExtractToFolder(archive.clone()));
        until_idle(&mut app);
        let restored = std::fs::read_to_string(dir.join("Bilder").join("Bilder").join("notiz.txt")).ok();
        lines.push(format!("extract to folder: content restored {}", restored.as_deref() == Some(content.as_str())));
        let mut app = App::new(Start::ExtractHere(archive.clone()));
        until_idle(&mut app);
        let here = std::fs::read_to_string(dir.join("Bilder").join("notiz.txt")).ok();
        lines.push(format!("extract here: content restored {}", here.as_deref() == Some(content.as_str())));

        let damaged = dir.join("Kaputt.qz");
        let options = quarz::ops::CreateOptions { level: 1, recovery: 10, ..Default::default() };
        std::fs::create_dir_all(dir.join("Daten")).unwrap();
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let noise: Vec<u8> = (0..3_000_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        std::fs::write(dir.join("Daten").join("daten.bin"), noise).unwrap();
        quarz::ops::create(&damaged, &[dir.join("Daten")], &options, &quarz::progress::Progress::default()).unwrap();
        let intact = std::fs::read(&damaged).unwrap();
        let repaired = damaged.with_file_name("Kaputt.repariert.qz");
        let mut bytes = intact.clone();
        let end = bytes.windows(4).position(|w| w == b"QZ2E").unwrap();
        bytes[end - 70..end - 50].iter_mut().for_each(|b| *b ^= 0x5a);
        std::fs::write(&damaged, &bytes).unwrap();
        let mut app = App::new(Start::Open(damaged.clone()));
        let opened = app.has_archive();
        let pressed = app.press_repair();
        until_idle(&mut app);
        let works = quarz::ops::open(&repaired, None).is_ok();
        lines.push(format!("broken table of contents: opened {opened}, repair offered {pressed}, repaired copy opens {works}"));
        let _ = std::fs::remove_file(&repaired);
        let mut bytes = intact.clone();
        bytes[100_000..120_000].iter_mut().for_each(|b| *b ^= 0x5a);
        std::fs::write(&damaged, &bytes).unwrap();
        let mut app = App::new(Start::Open(damaged.clone()));
        app.start_test();
        until_idle(&mut app);
        let failed = app.last_result().is_some_and(|o| !o.ok);
        let pressed = app.press_repair();
        until_idle(&mut app);
        let works = quarz::ops::open(&repaired, None).is_ok() && std::fs::read(&repaired).ok() == Some(intact.clone());
        lines.push(format!("broken data: check failed {failed}, repair offered {pressed}, repaired copy identical {works}"));
        let _ = std::fs::remove_dir_all(&dir);
        lines
    });
    let (code, text) = match result {
        Ok(lines) => (0, format!("SMOKE OK\n{}\n", lines.join("\n"))),
        Err(_) => (1, "SMOKE FAILED: panic\n".to_string()),
    };
    let _ = std::fs::write(report, text);
    code
}

fn main() {
    quarz::lang::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = |from: usize| -> Vec<PathBuf> {
        match args.get(from).map(String::as_str) {
            Some("--list") => {
                let Some(list) = args.get(from + 1) else { return Vec::new() };
                let text = std::fs::read_to_string(list).unwrap_or_default();
                let _ = std::fs::remove_file(list);
                text.lines().filter(|l| !l.trim().is_empty()).map(PathBuf::from).collect()
            }
            _ => args[from..].iter().map(PathBuf::from).collect(),
        }
    };
    let start = match args.first().map(String::as_str) {
        None => match ops::embedded_archive() {
            Some(archive) => Start::SelfExtract(archive),
            None => Start::Home,
        },
        Some("--smoke") => std::process::exit(smoke(&args[1..])),
        #[cfg(windows)]
        Some("--install") => {
            install(args.iter().any(|a| a == "--quiet"));
            return;
        }
        #[cfg(windows)]
        Some("--uninstall") => {
            let exe = std::env::current_exe().unwrap_or_default();
            shell::uninstall_modern(exe.parent().unwrap_or(Path::new(".")));
            let _ = shell::uninstall();
            message("Quarz", tr("Die Explorer-Einträge von Quarz wurden entfernt."));
            return;
        }
        #[cfg(windows)]
        Some("--classic-menu") => {
            let on = args.get(1).map(String::as_str) != Some("off");
            match shell::classic_menu(on) {
                Ok(()) if on => message("Quarz", tr("Windows zeigt ab der nächsten Anmeldung wieder das vollständige Rechtsklick-Menü.")),
                Ok(()) => message("Quarz", tr("Windows zeigt ab der nächsten Anmeldung wieder das kurze Rechtsklick-Menü.")),
                Err(e) => message("Quarz", &format!("{}: {e}", tr("Das hat nicht geklappt"))),
            }
            return;
        }
        Some(flag @ ("--add" | "--pack-now")) => {
            let Some(inputs) = collect_paths(paths(1), flag.trim_start_matches('-')) else { return };
            if inputs.is_empty() {
                return;
            }
            if flag == "--add" { Start::PackDialog(inputs) } else { Start::PackNow(inputs) }
        }
        Some("--extract-here") if args.len() > 1 => Start::ExtractHere(PathBuf::from(&args[1])),
        Some("--extract-to") if args.len() > 1 => Start::ExtractToFolder(PathBuf::from(&args[1])),
        Some("--extract") if args.len() > 1 => Start::ExtractDialog(PathBuf::from(&args[1])),
        Some(path) => {
            let path = PathBuf::from(path);
            if path.is_file() { Start::Open(path) } else { Start::Home }
        }
    };

    let app = App::new(start);
    let size = app.window_size();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Quarz")
        .with_inner_size(size)
        .with_min_inner_size([420.0, 150.0])
        .with_drag_and_drop(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../../assets/quarz256.png")) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let result = eframe::run_native(
        "Quarz",
        options,
        Box::new(move |cc| {
            gui::setup(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    );
    if let Err(e) = result {
        message("Quarz", &format!("{} {e}", tr("Das Fenster konnte nicht geöffnet werden:")));
    }
}
