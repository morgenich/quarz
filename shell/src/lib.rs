#![allow(non_snake_case)]

use std::cell::Cell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicPtr, Ordering};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_NOTIMPL, HINSTANCE, HMODULE, S_FALSE, S_OK,
};
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx, IClassFactory, IClassFactory_Impl};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::UI::Shell::{
    ECF_DEFAULT, ECF_HASSUBCOMMANDS, ECS_ENABLED, ECS_HIDDEN, IEnumExplorerCommand, IEnumExplorerCommand_Impl,
    IExplorerCommand, IExplorerCommand_Impl, IShellItemArray, SHStrDupW, SIGDN_FILESYSPATH,
};
use windows::core::{BOOL, GUID, HRESULT, HSTRING, IUnknown, Interface, PWSTR, Ref, Result, implement};

const CLSID_QUARZ: GUID = GUID::from_u128(0x6a1f0c2e_5b7d_4e3a_9c41_8f2d7b6e5a10);

const ARCHIVES: &[&str] = &["qz", "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "tbz2", "xz", "txz", "zst", "tzst"];

static MODULE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Root,
    PackNow,
    PackOptions,
    ExtractHere,
    ExtractToFolder,
    Open,
}

#[implement(IExplorerCommand)]
struct Command {
    kind: Kind,
}

fn program() -> PathBuf {
    let mut buffer = [0u16; 1024];
    let module = HMODULE(MODULE.load(Ordering::Relaxed));
    let len = unsafe { GetModuleFileNameW(Some(module), &mut buffer) } as usize;
    PathBuf::from(String::from_utf16_lossy(&buffer[..len])).with_file_name("quarz.exe")
}

fn selected(items: &Ref<IShellItemArray>) -> Vec<PathBuf> {
    let Some(items) = items.as_ref() else { return Vec::new() };
    let mut paths = Vec::new();
    unsafe {
        for index in 0..items.GetCount().unwrap_or(0) {
            let Ok(item) = items.GetItemAt(index) else { continue };
            let Ok(name) = item.GetDisplayName(SIGDN_FILESYSPATH) else { continue };
            if let Ok(text) = name.to_string() {
                paths.push(PathBuf::from(text));
            }
            CoTaskMemFree(Some(name.0 as *const c_void));
        }
    }
    paths
}

fn is_archive(path: &Path) -> bool {
    path.is_file() && path.extension().is_some_and(|e| ARCHIVES.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

fn stem(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    stem.strip_suffix(".tar").map(str::to_string).unwrap_or(stem)
}

fn short(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= 28 {
        return name.to_string();
    }
    let head: String = chars[..14].iter().collect();
    let tail: String = chars[chars.len() - 10..].iter().collect();
    format!("{head}…{tail}")
}

fn archive_name(paths: &[PathBuf]) -> String {
    match paths {
        [one] => stem(one),
        [first, ..] => first
            .parent()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Archiv".into()),
        [] => "Archiv".into(),
    }
}

fn text(value: &str) -> Result<PWSTR> {
    unsafe { SHStrDupW(&HSTRING::from(value)) }
}

fn language() -> usize {
    let from_code = |code: &str| {
        let code = code.trim().to_lowercase();
        ["de", "en", "es", "fr"].iter().position(|l| code.starts_with(l))
    };
    if let Some(forced) = std::env::var("QUARZ_LANG").ok().and_then(|code| from_code(&code)) {
        return forced;
    }
    let saved = std::env::var_os("APPDATA")
        .map(|dir| PathBuf::from(dir).join("Quarz").join("sprache.txt"))
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|code| from_code(&code));
    if let Some(saved) = saved {
        return saved;
    }
    let mut buffer = [0u16; 85];
    let len = unsafe { GetUserDefaultLocaleName(&mut buffer) };
    if len > 1 {
        return from_code(&String::from_utf16_lossy(&buffer[..len as usize - 1])).unwrap_or(1);
    }
    1
}

fn pick(texts: [&str; 4]) -> &str {
    texts[language()]
}

fn launch(flag: &str, paths: &[PathBuf]) {
    let mut command = std::process::Command::new(program());
    if !flag.is_empty() {
        command.arg(flag);
    }
    let length: usize = paths.iter().map(|p| p.as_os_str().len() + 3).sum();
    if paths.len() > 1 && length > 6000 {
        let list = std::env::temp_dir().join(format!("quarz-auswahl-{}.txt", std::process::id()));
        let lines: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        if std::fs::write(&list, lines.join("\n")).is_ok() {
            command.arg("--list").arg(list);
        }
    } else {
        command.args(paths);
    }
    let _ = command.spawn();
}

impl IExplorerCommand_Impl for Command_Impl {
    fn GetTitle(&self, items: Ref<IShellItemArray>) -> Result<PWSTR> {
        let paths = selected(&items);
        let title = match self.kind {
            Kind::Root => "Quarz".to_string(),
            Kind::PackNow => pick(["Zu „{}.qz“ packen", "Pack to “{}.qz”", "Comprimir en «{}.qz»", "Compresser vers « {}.qz »"])
                .replace("{}", &short(&archive_name(&paths))),
            Kind::PackOptions => {
                pick(["Packen mit Optionen …", "Pack with options…", "Comprimir con opciones…", "Compresser avec des options…"]).to_string()
            }
            Kind::ExtractHere => pick(["Hier entpacken", "Extract here", "Extraer aquí", "Extraire ici"]).to_string(),
            Kind::ExtractToFolder => match paths.as_slice() {
                [one] => pick([
                    "In Ordner „{}“ entpacken",
                    "Extract to folder “{}”",
                    "Extraer en la carpeta «{}»",
                    "Extraire dans le dossier « {} »",
                ])
                .replace("{}", &short(&stem(one))),
                _ => pick([
                    "Jedes in einen eigenen Ordner entpacken",
                    "Extract each to its own folder",
                    "Extraer cada uno en su propia carpeta",
                    "Extraire chacune dans son propre dossier",
                ])
                .to_string(),
            },
            Kind::Open => pick(["Mit Quarz öffnen", "Open with Quarz", "Abrir con Quarz", "Ouvrir avec Quarz"]).to_string(),
        };
        text(&title)
    }

    fn GetIcon(&self, _items: Ref<IShellItemArray>) -> Result<PWSTR> {
        text(&format!("{},0", program().display()))
    }

    fn GetToolTip(&self, _items: Ref<IShellItemArray>) -> Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }

    fn GetCanonicalName(&self) -> Result<GUID> {
        Ok(GUID::zeroed())
    }

    fn GetState(&self, items: Ref<IShellItemArray>, _ok_to_be_slow: BOOL) -> Result<u32> {
        let paths = selected(&items);
        let archives = !paths.is_empty() && paths.iter().all(|p| is_archive(p));
        let visible = match self.kind {
            Kind::Root | Kind::PackNow | Kind::PackOptions => !paths.is_empty(),
            Kind::ExtractHere | Kind::ExtractToFolder => archives,
            Kind::Open => archives && paths.len() == 1,
        };
        Ok(if visible { ECS_ENABLED.0 } else { ECS_HIDDEN.0 } as u32)
    }

    fn Invoke(&self, items: Ref<IShellItemArray>, _context: Ref<IBindCtx>) -> Result<()> {
        let paths = selected(&items);
        if paths.is_empty() {
            return Ok(());
        }
        match self.kind {
            Kind::Root => {}
            Kind::PackNow => launch("--pack-now", &paths),
            Kind::PackOptions => launch("--add", &paths),
            Kind::ExtractHere => paths.iter().for_each(|p| launch("--extract-here", std::slice::from_ref(p))),
            Kind::ExtractToFolder => paths.iter().for_each(|p| launch("--extract-to", std::slice::from_ref(p))),
            Kind::Open => launch("", &paths[..1]),
        }
        Ok(())
    }

    fn GetFlags(&self) -> Result<u32> {
        Ok(if self.kind == Kind::Root { ECF_HASSUBCOMMANDS.0 } else { ECF_DEFAULT.0 } as u32)
    }

    fn EnumSubCommands(&self) -> Result<IEnumExplorerCommand> {
        if self.kind != Kind::Root {
            return Err(E_NOTIMPL.into());
        }
        let items = [Kind::ExtractHere, Kind::ExtractToFolder, Kind::Open, Kind::PackNow, Kind::PackOptions]
            .into_iter()
            .map(|kind| Command { kind }.into())
            .collect();
        Ok(Entries { items, position: Cell::new(0) }.into())
    }
}

#[implement(IEnumExplorerCommand)]
struct Entries {
    items: Vec<IExplorerCommand>,
    position: Cell<usize>,
}

impl IEnumExplorerCommand_Impl for Entries_Impl {
    fn Next(&self, count: u32, out: *mut Option<IExplorerCommand>, fetched: *mut u32) -> HRESULT {
        let mut given = 0usize;
        unsafe {
            while given < count as usize && self.position.get() < self.items.len() {
                out.add(given).write(Some(self.items[self.position.get()].clone()));
                self.position.set(self.position.get() + 1);
                given += 1;
            }
            if !fetched.is_null() {
                *fetched = given as u32;
            }
        }
        if given == count as usize { S_OK } else { S_FALSE }
    }

    fn Skip(&self, count: u32) -> Result<()> {
        self.position.set((self.position.get() + count as usize).min(self.items.len()));
        Ok(())
    }

    fn Reset(&self) -> Result<()> {
        self.position.set(0);
        Ok(())
    }

    fn Clone(&self) -> Result<IEnumExplorerCommand> {
        Ok(Entries { items: self.items.clone(), position: Cell::new(self.position.get()) }.into())
    }
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, iid: *const GUID, object: *mut *mut c_void) -> Result<()> {
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let command: IExplorerCommand = Command { kind: Kind::Root }.into();
        unsafe { command.query(iid, object).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == 1 {
        MODULE.store(module.0, Ordering::Relaxed);
    }
    true.into()
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(class: *const GUID, iid: *const GUID, object: *mut *mut c_void) -> HRESULT {
    unsafe {
        if class.is_null() || *class != CLSID_QUARZ {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = Factory.into();
        factory.query(iid, object)
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}
