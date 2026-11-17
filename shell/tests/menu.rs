use std::ffi::c_void;
use std::path::{Path, PathBuf};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, IBindCtx, IClassFactory};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Shell::{
    ECF_HASSUBCOMMANDS, ECS_ENABLED, IExplorerCommand, IShellItem, IShellItemArray, SHCreateItemFromParsingName,
    SHCreateShellItemArrayFromShellItem,
};
use windows::core::{GUID, HRESULT, HSTRING, Interface, PWSTR, s};

const CLSID_QUARZ: GUID = GUID::from_u128(0x6a1f0c2e_5b7d_4e3a_9c41_8f2d7b6e5a10);

type GetClassObject = unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;

unsafe fn take(text: PWSTR) -> String {
    unsafe {
        let value = text.to_string().unwrap();
        CoTaskMemFree(Some(text.0 as *const c_void));
        value
    }
}

unsafe fn array_of(path: &Path) -> IShellItemArray {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None::<&IBindCtx>).unwrap();
        SHCreateShellItemArrayFromShellItem(&item).unwrap()
    }
}

unsafe fn entries(root: &IExplorerCommand, path: &Path) -> Vec<String> {
    unsafe {
        let items = array_of(path);
        let list = root.EnumSubCommands().unwrap();
        let mut titles = Vec::new();
        loop {
            let mut slot = [None];
            let mut fetched = 0u32;
            let _ = list.Next(&mut slot, Some(&mut fetched));
            let Some(command) = slot[0].take() else { break };
            if command.GetState(&items, false).unwrap() == ECS_ENABLED.0 as u32 {
                titles.push(take(command.GetTitle(&items).unwrap()));
            }
        }
        titles
    }
}

#[test]
fn menu_entries() {
    let library: PathBuf = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("quarz_shell.dll");
    assert!(library.is_file(), "{} fehlt", library.display());
    let dir = std::env::temp_dir().join(format!("quarz-menue-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("Urlaub 2026")).unwrap();
    std::fs::write(dir.join("Notiz.txt"), "hallo").unwrap();
    std::fs::write(dir.join("Paket.zip"), "PK").unwrap();

    unsafe {
        std::env::set_var("QUARZ_LANG", "de");
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
        let module = LoadLibraryW(&HSTRING::from(library.as_path())).unwrap();
        let entry = GetProcAddress(module, s!("DllGetClassObject")).expect("DllGetClassObject");
        let get: GetClassObject = std::mem::transmute(entry);

        let mut raw = std::ptr::null_mut();
        assert!(get(&GUID::zeroed(), &IClassFactory::IID, &mut raw).is_err());

        get(&CLSID_QUARZ, &IClassFactory::IID, &mut raw).ok().unwrap();
        let factory = IClassFactory::from_raw(raw);
        let root: IExplorerCommand = factory.CreateInstance(None).unwrap();

        let file = array_of(&dir.join("Notiz.txt"));
        assert_eq!(take(root.GetTitle(&file).unwrap()), "Quarz");
        assert_eq!(root.GetFlags().unwrap(), ECF_HASSUBCOMMANDS.0 as u32);
        let icon = take(root.GetIcon(&file).unwrap());
        assert!(icon.ends_with("quarz.exe,0"), "{icon}");

        let for_file = entries(&root, &dir.join("Notiz.txt"));
        assert_eq!(for_file, ["Zu „Notiz.qz“ packen", "Packen mit Optionen …"]);

        let for_folder = entries(&root, &dir.join("Urlaub 2026"));
        assert_eq!(for_folder, ["Zu „Urlaub 2026.qz“ packen", "Packen mit Optionen …"]);

        let for_archive = entries(&root, &dir.join("Paket.zip"));
        assert_eq!(
            for_archive,
            ["Hier entpacken", "In Ordner „Paket“ entpacken", "Mit Quarz öffnen", "Zu „Paket.qz“ packen", "Packen mit Optionen …"]
        );
        println!("Datei:  {for_file:?}\nOrdner: {for_folder:?}\nArchiv: {for_archive:?}");

        std::env::set_var("QUARZ_LANG", "en");
        let english = entries(&root, &dir.join("Paket.zip"));
        assert_eq!(
            english,
            ["Extract here", "Extract to folder “Paket”", "Open with Quarz", "Pack to “Paket.qz”", "Pack with options…"]
        );
        println!("English: {english:?}");
    }
    let _ = std::fs::remove_dir_all(dir);
}
