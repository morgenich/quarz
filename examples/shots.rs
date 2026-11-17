use eframe::egui;
use quarz::gui::{self, App, Start};
use quarz::lang::{self, Lang, trf};
use quarz::ops::{self, CreateOptions};
use quarz::progress::Progress;
use std::path::{Path, PathBuf};

fn shot(out: &Path, wanted: &[String], name: &str, size: [f32; 2], theme: egui::Theme, app: &mut App) {
    if !wanted.is_empty() && !wanted.iter().any(|w| name.contains(w.as_str())) {
        return;
    }
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(size[0], size[1]))
        .with_pixels_per_point(1.0)
        .with_theme(theme)
        .with_accessibility_check(false)
        .wgpu()
        .with_context_setup(gui::setup)
        .build_ui(|ui| app.draw(ui));
    harness.run_steps(4);
    match harness.render() {
        Ok(image) => {
            let (w, h) = (image.width() - 16, image.height() - 16);
            let cropped = image::imageops::crop_imm(&image, 8, 8, w, h).to_image();
            let path = out.join(format!("{name}.png"));
            cropped.save(&path).expect("save png");
            println!("{}", path.display());
        }
        Err(e) => println!("{name}: {e}"),
    }
}

fn main() {
    unsafe { std::env::set_var("QUARZ_NO_RECENT", "1") };
    let mut args = std::env::args().skip(1);
    let base = PathBuf::from(args.next().expect("output folder"));
    let language = args.next().and_then(|code| Lang::from_code(&code)).unwrap_or(Lang::De);
    let wanted: Vec<String> = args.collect();
    lang::set(language);
    let out = base.join(language.code());
    std::fs::create_dir_all(&out).unwrap();
    let german = language == Lang::De;

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let archive = out.join(if german { "Projekt.qz" } else { "Project.qz" });
    let _ = std::fs::remove_file(&archive);
    let options = CreateOptions { level: 3, comment: if german { "Stand vom Montag" } else { "Monday's state" }.into(), ..Default::default() };
    ops::create(&archive, &[root.join("src"), root.join("Cargo.toml")], &options, &Progress::default()).expect("demo archive");

    let (light, dark) = (egui::Theme::Light, egui::Theme::Dark);
    let big = [976.0, 656.0];
    let home = "C:\\Users\\Alex";
    let photos = if german { "Urlaubsfotos" } else { "Holiday photos" };
    let recent = vec![
        PathBuf::from(format!("{home}\\Desktop\\{photos}.qz")),
        PathBuf::from(format!("{home}\\Downloads\\Minecraft.qz")),
        PathBuf::from(format!("{home}\\Downloads\\Mods.zip")),
    ];
    let inputs = vec![
        PathBuf::from(format!("{home}\\Desktop\\{photos}")),
        PathBuf::from(format!("{home}\\Desktop\\{}", if german { "Notizen.txt" } else { "Notes.txt" })),
    ];

    let mut app = App::new(Start::Home);
    app.preview_recent(recent.clone());
    shot(&out, &wanted, "01-start", big, light, &mut app);
    shot(&out, &wanted, "02-start-dark", big, dark, &mut app);

    let mut app = App::new(Start::Open(archive.clone()));
    shot(&out, &wanted, "03-archive", big, light, &mut app);
    app.preview_enter("src");
    app.preview_select(&[2, 3]);
    shot(&out, &wanted, "04-archive-selection", big, light, &mut app);
    shot(&out, &wanted, "05-archive-dark", big, dark, &mut app);

    let mut app = App::new(Start::Home);
    app.preview_recent(recent.clone());
    app.preview_pack(inputs.clone(), false);
    shot(&out, &wanted, "06-pack", big, light, &mut app);
    app.preview_pack(inputs.clone(), true);
    shot(&out, &wanted, "07-pack-options", big, light, &mut app);

    let mut app = App::new(Start::Open(archive.clone()));
    app.preview_extract(Some(&format!("{home}\\Desktop\\{}", if german { "Projekt" } else { "Project" })));
    shot(&out, &wanted, "08-extract", big, light, &mut app);

    let mut app = App::new(Start::Open(archive.clone()));
    app.preview_job(&trf("Packe {name}", &[("name", &format!("{photos}.qz"))]), 0.62);
    shot(&out, &wanted, "09-progress", big, light, &mut app);

    let mut app = App::new(Start::Open(archive.clone()));
    let line = trf(
        "{n} Dateien, {from} → {to} ({p} % kleiner)",
        &[("n", "1406"), ("from", &lang::size(117_100_000)), ("to", &lang::size(52_600_000)), ("p", "55")],
    );
    app.preview_result(App::preview_outcome(lang::tr("Fertig gepackt"), &[&line], true, Some(archive.clone())));
    shot(&out, &wanted, "10-done", big, light, &mut app);

    let mut app = App::new(Start::Home);
    app.preview_recent(recent.clone());
    app.preview_password(PathBuf::from(if german { "Geheim.qz" } else { "Secret.qz" }), true);
    shot(&out, &wanted, "11-password", big, light, &mut app);

    let mut app = App::new(Start::Open(archive.clone()));
    app.preview_details();
    shot(&out, &wanted, "12-properties", big, light, &mut app);

    let mut app = App::new(Start::Home);
    app.preview_compact(true);
    app.preview_job(&trf("Packe {name}", &[("name", &format!("{photos}.qz"))]), 0.35);
    shot(&out, &wanted, "13-small-progress", [516.0, 136.0], light, &mut app);

    let mut app = App::new(Start::PackDialog(inputs));
    shot(&out, &wanted, "14-small-pack", [576.0, 446.0], light, &mut app);
    let _ = std::fs::remove_file(&archive);
}
