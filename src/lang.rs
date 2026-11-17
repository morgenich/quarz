use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    De = 0,
    En = 1,
    Es = 2,
    Fr = 3,
}

impl Lang {
    pub const ALL: [Lang; 4] = [Lang::De, Lang::En, Lang::Es, Lang::Fr];

    pub fn name(self) -> &'static str {
        ["Deutsch", "English", "Español", "Français"][self as usize]
    }

    pub fn code(self) -> &'static str {
        ["de", "en", "es", "fr"][self as usize]
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        let code = code.trim().to_lowercase();
        Lang::ALL.into_iter().find(|l| code.starts_with(l.code()))
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn current() -> Lang {
    Lang::ALL[CURRENT.load(Ordering::Relaxed) as usize % Lang::ALL.len()]
}

pub fn set(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

fn settings_file() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("Quarz").join("sprache.txt"))
}

pub fn save(lang: Lang) {
    set(lang);
    if let Some(file) = settings_file() {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, lang.code());
    }
}

pub fn system() -> Lang {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
        let mut buffer = [0u16; 85];
        let len = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
        if len > 1 {
            let name = String::from_utf16_lossy(&buffer[..len as usize - 1]);
            return Lang::from_code(&name).unwrap_or(Lang::En);
        }
    }
    Lang::En
}

pub fn init() {
    let forced = std::env::var("QUARZ_LANG").ok().and_then(|code| Lang::from_code(&code));
    let saved = || settings_file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|code| Lang::from_code(&code));
    set(forced.or_else(saved).unwrap_or_else(system));
}

pub fn tr(german: &'static str) -> &'static str {
    let lang = current();
    if lang == Lang::De {
        return german;
    }
    static INDEX: OnceLock<HashMap<&'static str, &'static [&'static str; 3]>> = OnceLock::new();
    let index = INDEX.get_or_init(|| TABLE.iter().map(|(key, texts)| (*key, texts)).collect());
    index.get(german).map(|texts| texts[lang as usize - 1]).unwrap_or(german)
}

pub fn trf(german: &'static str, values: &[(&str, &str)]) -> String {
    let mut text = tr(german).to_string();
    for (name, value) in values {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

pub fn size(bytes: u64) -> String {
    let text = crate::ops::format_size(bytes);
    if current() == Lang::En { text.replace(',', ".") } else { text }
}

pub fn time(nanos: i64) -> String {
    let Some([year, month, day, hour, minute]) = crate::ops::local_time(nanos) else { return String::new() };
    match current() {
        Lang::De => format!("{day:02}.{month:02}.{year} {hour:02}:{minute:02}"),
        Lang::En => format!("{year}-{month:02}-{day:02} {hour:02}:{minute:02}"),
        Lang::Es | Lang::Fr => format!("{day:02}/{month:02}/{year} {hour:02}:{minute:02}"),
    }
}

pub fn level_name(id: u8) -> &'static str {
    const NAMES: [[&str; 4]; 6] = [
        ["Nur speichern", "Store only", "Solo almacenar", "Stocker seulement"],
        ["Am schnellsten", "Fastest", "El más rápido", "Le plus rapide"],
        ["Schnell", "Fast", "Rápido", "Rapide"],
        ["Normal", "Normal", "Normal", "Normal"],
        ["Gut", "Good", "Bueno", "Bon"],
        ["Beste", "Best", "El mejor", "Le meilleur"],
    ];
    NAMES[(id as usize).min(5)][current() as usize]
}

pub fn error(text: &str) -> String {
    const KNOWN: &[(&str, [&str; 4])] = &[
        ("abgebrochen", ["Abgebrochen.", "Cancelled.", "Cancelado.", "Annulé."]),
        ("falsches Passwort", ["Das Passwort stimmt nicht.", "Wrong password.", "Contraseña incorrecta.", "Mot de passe incorrect."]),
        ("Passwort noetig", ["Ein Passwort ist nötig.", "A password is required.", "Se necesita una contraseña.", "Un mot de passe est nécessaire."]),
        ("nichts zu packen", ["Es gibt nichts zu packen.", "There is nothing to pack.", "No hay nada que comprimir.", "Il n'y a rien à compresser."]),
        (
            "ist kein bekanntes Archiv",
            ["Das ist kein Archiv, das Quarz kennt.", "This is not an archive Quarz knows.", "No es un archivo que Quarz conozca.", "Ce n'est pas une archive que Quarz connaît."],
        ),
        (
            "gesperrt und kann nicht geaendert werden",
            ["Das Archiv ist gesperrt und kann nicht geändert werden.", "The archive is locked and cannot be changed.", "El archivo está bloqueado y no se puede modificar.", "L'archive est verrouillée et ne peut pas être modifiée."],
        ),
        (
            "mehrteilige Archive koennen nicht geaendert werden",
            ["Aufgeteilte Archive können nicht geändert werden.", "Split archives cannot be changed.", "Los archivos divididos no se pueden modificar.", "Les archives divisées ne peuvent pas être modifiées."],
        ),
        (
            "enthaelt keine Wiederherstellungsdaten",
            ["Das Archiv enthält keine Schutzdaten zum Reparieren.", "The archive has no protection data to repair it with.", "El archivo no tiene datos de protección para repararlo.", "L'archive ne contient pas de données de protection pour la réparer."],
        ),
        (
            "Teile muessen mindestens 64 KB gross sein",
            ["Teile müssen mindestens 64 KB groß sein.", "Parts must be at least 64 KB.", "Las partes deben tener al menos 64 KB.", "Les parties doivent faire au moins 64 Ko."],
        ),
        (
            "Archiv ist beschaedigt",
            ["Das Archiv ist beschädigt.", "The archive is damaged.", "El archivo está dañado.", "L'archive est endommagée."],
        ),
    ];
    for (needle, texts) in KNOWN {
        if text.contains(needle) {
            return texts[current() as usize].to_string();
        }
    }
    text.to_string()
}

#[rustfmt::skip]
const TABLE: &[(&str, [&str; 3])] = &[
    ("Dateien kleiner packen und Archive öffnen", ["Make files smaller and open archives", "Comprime archivos y abre archivos comprimidos", "Compressez vos fichiers et ouvrez des archives"]),
    ("Dateien oder Ordner hierher ziehen", ["Drop files or folders here", "Arrastra archivos o carpetas aquí", "Déposez des fichiers ou des dossiers ici"]),
    ("zum Packen – oder ein Archiv, um es zu öffnen", ["to pack them – or an archive to open it", "para comprimirlos, o un archivo comprimido para abrirlo", "pour les compresser – ou une archive pour l'ouvrir"]),
    ("Dateien packen", ["Pack files", "Comprimir archivos", "Compresser des fichiers"]),
    ("auswählen und verkleinern", ["choose and shrink", "elegir y reducir", "choisir et réduire"]),
    ("Ordner packen", ["Pack a folder", "Comprimir carpeta", "Compresser un dossier"]),
    ("einen ganzen Ordner", ["a whole folder", "una carpeta entera", "un dossier entier"]),
    ("Archiv öffnen", ["Open archive", "Abrir archivo", "Ouvrir une archive"]),
    ("ansehen und entpacken", ["view and extract", "ver y extraer", "voir et extraire"]),
    ("Zuletzt geöffnet", ["Recently opened", "Abiertos recientemente", "Ouverts récemment"]),
    ("Dateien zum Packen auswählen", ["Choose files to pack", "Elige archivos para comprimir", "Choisir les fichiers à compresser"]),
    ("Ordner zum Packen auswählen", ["Choose a folder to pack", "Elige una carpeta para comprimir", "Choisir le dossier à compresser"]),
    ("Archive", ["Archives", "Archivos comprimidos", "Archives"]),
    ("Alle Dateien", ["All files", "Todos los archivos", "Tous les fichiers"]),
    ("Sprache", ["Language", "Idioma", "Langue"]),
    ("Zur Startseite", ["Back to start", "Volver al inicio", "Retour à l'accueil"]),
    ("{n} Dateien", ["{n} files", "{n} archivos", "{n} fichiers"]),
    ("im Archiv {size}", ["{size} in the archive", "{size} en el archivo", "{size} dans l'archive"]),
    ("Alles entpacken", ["Extract all", "Extraer todo", "Tout extraire"]),
    ("Auswahl entpacken ({n})", ["Extract selection ({n})", "Extraer selección ({n})", "Extraire la sélection ({n})"]),
    ("Hinzufügen", ["Add", "Añadir", "Ajouter"]),
    ("Löschen", ["Delete", "Eliminar", "Supprimer"]),
    ("Erst Einträge markieren", ["Select entries first", "Selecciona primero algún elemento", "Sélectionnez d'abord des éléments"]),
    ("Prüfen", ["Check", "Comprobar", "Vérifier"]),
    ("Kontrolliert, ob das Archiv unbeschädigt ist", ["Checks that the archive is not damaged", "Comprueba que el archivo no esté dañado", "Vérifie que l'archive n'est pas endommagée"]),
    ("Mehr", ["More", "Más", "Plus"]),
    ("Eigenschaften und Schutz …", ["Properties and protection…", "Propiedades y protección…", "Propriétés et protection…"]),
    ("Beschädigtes Archiv reparieren", ["Repair damaged archive", "Reparar archivo dañado", "Réparer une archive endommagée"]),
    ("Im Explorer anzeigen", ["Show in Explorer", "Mostrar en el Explorador", "Afficher dans l'Explorateur"]),
    ("{n} markiert", ["{n} selected", "{n} seleccionados", "{n} sélectionnés"]),
    ("Doppelklick öffnet eine Datei. Dateien hierher ziehen, um sie hinzuzufügen.", ["Double-click opens a file. Drop files here to add them.", "Doble clic abre un archivo. Arrastra archivos aquí para añadirlos.", "Double-cliquez pour ouvrir un fichier. Déposez des fichiers ici pour les ajouter."]),
    ("Doppelklick öffnet eine Datei.", ["Double-click opens a file.", "Doble clic abre un archivo.", "Double-cliquez pour ouvrir un fichier."]),
    ("Dieser Ordner ist leer.", ["This folder is empty.", "Esta carpeta está vacía.", "Ce dossier est vide."]),
    ("Name", ["Name", "Nombre", "Nom"]),
    ("Größe", ["Size", "Tamaño", "Taille"]),
    ("Geändert", ["Modified", "Modificado", "Modifié"]),
    ("verschlüsselt", ["encrypted", "cifrado", "chiffrée"]),
    ("{n} Teile", ["{n} parts", "{n} partes", "{n} parties"]),
    ("Schutz {p} %", ["protection {p} %", "protección {p} %", "protection {p} %"]),
    ("selbstentpackend", ["self-extracting", "autoextraíble", "auto-extractible"]),
    ("gesperrt", ["locked", "bloqueado", "verrouillée"]),
    ("Kompression: {name}", ["Compression: {name}", "Compresión: {name}", "Compression : {name}"]),
    ("Packe {name}", ["Packing {name}", "Comprimiendo {name}", "Compression de {name}"]),
    ("Entpacke {name}", ["Extracting {name}", "Extrayendo {name}", "Extraction de {name}"]),
    ("Prüfe {name}", ["Checking {name}", "Comprobando {name}", "Vérification de {name}"]),
    ("Füge zu {name} hinzu", ["Adding to {name}", "Añadiendo a {name}", "Ajout à {name}"]),
    ("Öffne {name}", ["Opening {name}", "Abriendo {name}", "Ouverture de {name}"]),
    ("Lösche aus dem Archiv", ["Deleting from the archive", "Eliminando del archivo", "Suppression dans l'archive"]),
    ("Repariere {name}", ["Repairing {name}", "Reparando {name}", "Réparation de {name}"]),
    ("Speichere Kommentar", ["Saving comment", "Guardando comentario", "Enregistrement du commentaire"]),
    ("Sperre Archiv", ["Locking archive", "Bloqueando archivo", "Verrouillage de l'archive"]),
    ("Berechne Schutzdaten", ["Computing protection data", "Calculando datos de protección", "Calcul des données de protection"]),
    ("Erzeuge selbstentpackende Datei", ["Creating self-extracting file", "Creando archivo autoextraíble", "Création du fichier auto-extractible"]),
    ("Füge Teile zusammen", ["Joining parts", "Uniendo partes", "Assemblage des parties"]),
    ("Wird abgebrochen …", ["Cancelling…", "Cancelando…", "Annulation…"]),
    ("Abbrechen", ["Cancel", "Cancelar", "Annuler"]),
    ("Fertig entpackt", ["Extracted", "Extraído", "Extraction terminée"]),
    ("Mit Fehlern entpackt", ["Extracted with errors", "Extraído con errores", "Extrait avec des erreurs"]),
    ("{size} entpackt.", ["{size} extracted.", "{size} extraídos.", "{size} extraits."]),
    ("{n} Dateien mit Fehlern:", ["{n} files with errors:", "{n} archivos con errores:", "{n} fichiers en erreur :"]),
    ("Ziel: {path}", ["Destination: {path}", "Destino: {path}", "Destination : {path}"]),
    ("Fertig gepackt", ["Packed", "Comprimido", "Compression terminée"]),
    ("{n} Dateien, {from} → {to} ({p} % kleiner)", ["{n} files, {from} → {to} ({p} % smaller)", "{n} archivos, {from} → {to} ({p} % menos)", "{n} fichiers, {from} → {to} ({p} % de moins)"]),
    ("Übersprungen: {name}", ["Skipped: {name}", "Omitido: {name}", "Ignoré : {name}"]),
    ("Hinzugefügt", ["Added", "Añadido", "Ajouté"]),
    ("{n} Dateien hinzugefügt ({size}).", ["{n} files added ({size}).", "{n} archivos añadidos ({size}).", "{n} fichiers ajoutés ({size})."]),
    ("Prüfung abgeschlossen", ["Check finished", "Comprobación terminada", "Vérification terminée"]),
    ("{size} geprüft.", ["{size} checked.", "{size} comprobados.", "{size} vérifiés."]),
    ("Das Archiv ist in Ordnung.", ["The archive is fine.", "El archivo está en buen estado.", "L'archive est en bon état."]),
    ("Das Archiv hat Fehler", ["The archive has errors", "El archivo tiene errores", "L'archive contient des erreurs"]),
    ("Repariert", ["Repaired", "Reparado", "Réparée"]),
    ("{n} beschädigte Stellen gefunden und repariert.", ["{n} damaged spots found and repaired.", "{n} zonas dañadas encontradas y reparadas.", "{n} zones endommagées trouvées et réparées."]),
    ("Repariertes Archiv: {name}", ["Repaired archive: {name}", "Archivo reparado: {name}", "Archive réparée : {name}"]),
    ("Der Schaden ist größer, als die Schutzdaten ausgleichen können.", ["The damage is larger than the protection data can make up for.", "El daño es mayor de lo que los datos de protección pueden compensar.", "Les dégâts dépassent ce que les données de protection peuvent compenser."]),
    ("Gelöscht", ["Deleted", "Eliminado", "Supprimé"]),
    ("{n} Dateien entfernt.", ["{n} files removed.", "{n} archivos eliminados.", "{n} fichiers supprimés."]),
    ("Gespeichert", ["Saved", "Guardado", "Enregistré"]),
    ("Der Kommentar ist gespeichert.", ["The comment has been saved.", "El comentario se ha guardado.", "Le commentaire est enregistré."]),
    ("Gesperrt", ["Locked", "Bloqueado", "Verrouillée"]),
    ("Das Archiv kann jetzt nicht mehr geändert werden.", ["The archive can no longer be changed.", "El archivo ya no se puede modificar.", "L'archive ne peut plus être modifiée."]),
    ("Geschützt", ["Protected", "Protegido", "Protégée"]),
    ("{p} % Schutzdaten hinzugefügt. Kleinere Schäden lassen sich jetzt reparieren.", ["{p} % protection data added. Minor damage can now be repaired.", "{p} % de datos de protección añadidos. Ahora se pueden reparar daños menores.", "{p} % de données de protection ajoutées. Les petits dégâts peuvent maintenant être réparés."]),
    ("Fertig", ["Done", "Listo", "Terminé"]),
    ("Angelegt: {name}", ["Created: {name}", "Creado: {name}", "Créé : {name}"]),
    ("Zusammengefügt", ["Joined", "Unido", "Assemblé"]),
    ("{n} Teile wurden zu {name} zusammengefügt.", ["{n} parts were joined into {name}.", "{n} partes se unieron en {name}.", "{n} parties ont été assemblées en {name}."]),
    ("Abgebrochen", ["Cancelled", "Cancelado", "Annulé"]),
    ("Das hat nicht geklappt", ["That did not work", "No ha funcionado", "Cela n'a pas fonctionné"]),
    ("Der Vorgang ist abgestürzt.", ["The operation crashed.", "La operación ha fallado.", "L'opération a planté."]),
    ("Schließen", ["Close", "Cerrar", "Fermer"]),
    ("Reparieren", ["Repair", "Reparar", "Réparer"]),
    ("Das Archiv enthält Schutzdaten und lässt sich wahrscheinlich reparieren.", ["The archive carries protection data and can probably be repaired.", "El archivo tiene datos de protección y probablemente se pueda reparar.", "L'archive contient des données de protection et peut sans doute être réparée."]),
    ("Im Explorer zeigen", ["Show in Explorer", "Mostrar en el Explorador", "Afficher dans l'Explorateur"]),
    ("Passwort eingeben", ["Enter password", "Introduce la contraseña", "Saisir le mot de passe"]),
    ("{name} ist geschützt.", ["{name} is protected.", "{name} está protegido.", "{name} est protégée."]),
    ("Das Passwort stimmt nicht. Bitte noch einmal versuchen.", ["That password is not correct. Please try again.", "La contraseña no es correcta. Inténtalo de nuevo.", "Le mot de passe est incorrect. Veuillez réessayer."]),
    ("Passwort", ["Password", "Contraseña", "Mot de passe"]),
    ("Öffnen", ["Open", "Abrir", "Ouvrir"]),
    ("Aus dem Archiv löschen?", ["Delete from the archive?", "¿Eliminar del archivo?", "Supprimer de l'archive ?"]),
    ("„{name}“ wird aus dem Archiv entfernt.", ["“{name}” will be removed from the archive.", "«{name}» se eliminará del archivo.", "« {name} » sera supprimé de l'archive."]),
    ("{n} markierte Einträge werden aus dem Archiv entfernt.", ["{n} selected entries will be removed from the archive.", "Se eliminarán {n} elementos seleccionados del archivo.", "{n} éléments sélectionnés seront supprimés de l'archive."]),
    ("Die Dateien auf deinem Computer bleiben unberührt.", ["The files on your computer stay untouched.", "Los archivos de tu ordenador no se tocan.", "Les fichiers de votre ordinateur ne sont pas modifiés."]),
    ("Entpacken", ["Extract", "Extraer", "Extraire"]),
    ("Alles", ["Everything", "Todo", "Tout"]),
    ("Ordner {name}", ["Folder {name}", "Carpeta {name}", "Dossier {name}"]),
    ("{n} markierte Einträge", ["{n} selected entries", "{n} elementos seleccionados", "{n} éléments sélectionnés"]),
    ("Wohin?", ["Where to?", "¿Dónde?", "Où ?"]),
    ("Ändern …", ["Change…", "Cambiar…", "Modifier…"]),
    ("Zielordner", ["Destination folder", "Carpeta de destino", "Dossier de destination"]),
    ("Ordner danach im Explorer öffnen", ["Open the folder in Explorer afterwards", "Abrir la carpeta en el Explorador al terminar", "Ouvrir ensuite le dossier dans l'Explorateur"]),
    ("Vorhandene Dateien überschreiben", ["Overwrite existing files", "Sobrescribir archivos existentes", "Remplacer les fichiers existants"]),
    ("Selbstentpackendes Archiv", ["Self-extracting archive", "Archivo autoextraíble", "Archive auto-extractible"]),
    ("Packen", ["Pack", "Comprimir", "Compresser"]),
    ("nichts ausgewählt", ["nothing selected", "nada seleccionado", "rien de sélectionné"]),
    ("{name} und {n} weitere", ["{name} and {n} more", "{name} y {n} más", "{name} et {n} autres"]),
    ("Name des Archivs", ["Archive name", "Nombre del archivo", "Nom de l'archive"]),
    ("in {folder}", ["in {folder}", "en {folder}", "dans {folder}"]),
    ("Wo soll das Archiv gespeichert werden?", ["Where should the archive be saved?", "¿Dónde guardar el archivo?", "Où enregistrer l'archive ?"]),
    ("Wie stark verkleinern?", ["How much to shrink?", "¿Cuánto comprimir?", "Quel niveau de compression ?"]),
    ("Schnell", ["Fast", "Rápido", "Rapide"]),
    ("Normal", ["Normal", "Normal", "Normal"]),
    ("Klein", ["Small", "Pequeño", "Petit"]),
    ("Geht am schnellsten, wird etwas größer", ["Fastest, a little larger", "Lo más rápido, algo más grande", "Le plus rapide, un peu plus gros"]),
    ("Guter Mittelweg – empfohlen", ["A good balance – recommended", "Buen equilibrio: recomendado", "Bon compromis – recommandé"]),
    ("Dauert deutlich länger, wird am kleinsten", ["Takes much longer, smallest result", "Tarda bastante más, el resultado más pequeño", "Beaucoup plus long, résultat le plus petit"]),
    ("Passwort (wenn gewünscht)", ["Password (optional)", "Contraseña (opcional)", "Mot de passe (facultatif)"]),
    ("leer lassen für kein Passwort", ["leave empty for no password", "déjalo vacío para no usar contraseña", "laisser vide pour ne pas en mettre"]),
    ("anzeigen", ["show", "mostrar", "afficher"]),
    ("Weitere Optionen", ["More options", "Más opciones", "Plus d'options"]),
    ("In Teile aufteilen", ["Split into parts", "Dividir en partes", "Diviser en parties"]),
    ("nein", ["no", "no", "non"]),
    ("pro Teil", ["per part", "por parte", "par partie"]),
    ("Schutz gegen Beschädigung", ["Protection against damage", "Protección contra daños", "Protection contre les dégâts"]),
    ("Aus", ["Off", "No", "Non"]),
    ("Stark", ["Strong", "Fuerte", "Forte"]),
    ("Das Archiv wird 5 % größer und lässt sich bei kleinen Schäden reparieren", ["The archive gets 5 % larger and can be repaired after minor damage", "El archivo crece un 5 % y se puede reparar si sufre daños menores", "L'archive grossit de 5 % et peut être réparée après de petits dégâts"]),
    ("Das Archiv wird 10 % größer und verträgt mehr Schaden", ["The archive gets 10 % larger and survives more damage", "El archivo crece un 10 % y resiste más daños", "L'archive grossit de 10 % et résiste à davantage de dégâts"]),
    ("Selbstentpackend", ["Self-extracting", "Autoextraíble", "Auto-extractible"]),
    ("als .exe speichern, die sich selbst entpackt", ["save as an .exe that unpacks itself", "guardar como .exe que se extrae solo", "enregistrer en .exe qui s'extrait tout seul"]),
    ("Kommentar", ["Comment", "Comentario", "Commentaire"]),
    ("Genaue Stufe", ["Exact level", "Nivel exacto", "Niveau précis"]),
    ("„{name}“ gibt es schon. Bitte einen anderen Namen wählen.", ["“{name}” already exists. Please choose another name.", "«{name}» ya existe. Elige otro nombre.", "« {name} » existe déjà. Veuillez choisir un autre nom."]),
    ("Eigenschaften und Schutz", ["Properties and protection", "Propiedades y protección", "Propriétés et protection"]),
    ("Speichern", ["Save", "Guardar", "Enregistrer"]),
    ("Macht das Archiv etwas größer, dafür lässt es sich bei Schäden reparieren.", ["Makes the archive a little larger, but it can be repaired if it gets damaged.", "El archivo crece un poco, pero se puede reparar si se daña.", "L'archive grossit un peu, mais peut être réparée en cas de dégâts."]),
    ("5 % größer", ["5 % larger", "5 % más grande", "5 % plus gros"]),
    ("10 % größer", ["10 % larger", "10 % más grande", "10 % plus gros"]),
    ("Schutz hinzufügen", ["Add protection", "Añadir protección", "Ajouter la protection"]),
    ("Als selbstentpackende .exe speichern", ["Save as self-extracting .exe", "Guardar como .exe autoextraíble", "Enregistrer en .exe auto-extractible"]),
    ("Sperren", ["Lock", "Bloquear", "Verrouiller"]),
    ("Danach kann das Archiv nicht mehr geändert werden", ["After that the archive can no longer be changed", "Después el archivo ya no se podrá modificar", "Ensuite l'archive ne pourra plus être modifiée"]),
    ("Teile zu einer Datei zusammenfügen", ["Join the parts into one file", "Unir las partes en un solo archivo", "Assembler les parties en un seul fichier"]),
    ("Dateien zum Archiv hinzufügen", ["Add files to the archive", "Añadir archivos al archivo comprimido", "Ajouter des fichiers à l'archive"]),
    ("Quarz einrichten", ["Set up Quarz", "Configurar Quarz", "Configurer Quarz"]),
    ("Soll Quarz direkt im ersten Rechtsklick-Menü von Windows 11 erscheinen?\n\nWindows lässt dort nur signierte App-Pakete zu. Dafür wird das Zertifikat, mit dem das Quarz-Paket signiert ist, auf diesem Computer als vertrauenswürdig eingetragen. Windows fragt gleich nach Administrator-Bestätigung.\n\nBei „Nein“ stehen die Quarz-Einträge unter „Weitere Optionen anzeigen“.", [
        "Should Quarz appear directly in the first right-click menu of Windows 11?\n\nWindows only allows signed app packages there. For that, the certificate the Quarz package is signed with is added as trusted on this computer. Windows will ask for administrator approval next.\n\nWith “No”, the Quarz entries are found under “Show more options”.",
        "¿Debe aparecer Quarz directamente en el primer menú contextual de Windows 11?\n\nWindows solo admite ahí paquetes de aplicación firmados. Para ello, el certificado con el que está firmado el paquete de Quarz se añade como de confianza en este equipo. Windows pedirá a continuación la aprobación de un administrador.\n\nCon «No», las entradas de Quarz estarán en «Mostrar más opciones».",
        "Quarz doit-il apparaître directement dans le premier menu contextuel de Windows 11 ?\n\nWindows n'y accepte que des paquets d'application signés. Pour cela, le certificat qui signe le paquet Quarz est ajouté comme certificat de confiance sur cet ordinateur. Windows va demander l'accord d'un administrateur.\n\nAvec « Non », les entrées Quarz se trouvent sous « Afficher plus d'options ».",
    ]),
    ("Fertig. Rechtsklick auf Dateien, Ordner oder Archive zeigt jetzt direkt den Eintrag „Quarz“, und .qz-Dateien öffnen sich per Doppelklick.", [
        "Done. Right-clicking files, folders or archives now shows the “Quarz” entry directly, and .qz files open with a double-click.",
        "Listo. Al hacer clic derecho en archivos, carpetas o archivos comprimidos aparece directamente la entrada «Quarz», y los archivos .qz se abren con doble clic.",
        "Terminé. Un clic droit sur des fichiers, des dossiers ou des archives affiche maintenant directement l'entrée « Quarz », et les fichiers .qz s'ouvrent d'un double-clic.",
    ]),
    ("Fertig. Die Quarz-Einträge stehen im Rechtsklick-Menü unter „Weitere Optionen anzeigen“, und .qz-Dateien öffnen sich per Doppelklick.", [
        "Done. The Quarz entries are in the right-click menu under “Show more options”, and .qz files open with a double-click.",
        "Listo. Las entradas de Quarz están en el menú contextual, en «Mostrar más opciones», y los archivos .qz se abren con doble clic.",
        "Terminé. Les entrées Quarz se trouvent dans le menu contextuel sous « Afficher plus d'options », et les fichiers .qz s'ouvrent d'un double-clic.",
    ]),
    ("Das Windows-11-Menü konnte nicht eingerichtet werden:", ["The Windows 11 menu could not be set up:", "No se pudo configurar el menú de Windows 11:", "Le menu de Windows 11 n'a pas pu être configuré :"]),
    ("Eintragen fehlgeschlagen:", ["Setting up failed:", "La configuración ha fallado:", "La configuration a échoué :"]),
    ("Die Explorer-Einträge von Quarz wurden entfernt.", ["The Explorer entries of Quarz have been removed.", "Se han eliminado las entradas de Quarz del Explorador.", "Les entrées de Quarz dans l'Explorateur ont été supprimées."]),
    ("Windows zeigt ab der nächsten Anmeldung wieder das vollständige Rechtsklick-Menü.", ["From the next sign-in on, Windows shows the complete right-click menu again.", "A partir del próximo inicio de sesión, Windows volverá a mostrar el menú contextual completo.", "Dès la prochaine ouverture de session, Windows affichera de nouveau le menu contextuel complet."]),
    ("Windows zeigt ab der nächsten Anmeldung wieder das kurze Rechtsklick-Menü.", ["From the next sign-in on, Windows shows the short right-click menu again.", "A partir del próximo inicio de sesión, Windows volverá a mostrar el menú contextual corto.", "Dès la prochaine ouverture de session, Windows affichera de nouveau le menu contextuel court."]),
    ("Das Fenster konnte nicht geöffnet werden:", ["The window could not be opened:", "No se pudo abrir la ventana:", "La fenêtre n'a pas pu être ouverte :"]),
    ("Die Dateien für das Windows-11-Menü fehlen.", ["The files for the Windows 11 menu are missing.", "Faltan los archivos para el menú de Windows 11.", "Les fichiers du menu de Windows 11 sont manquants."]),
    ("Die Bestätigung wurde abgelehnt.", ["The approval was declined.", "Se rechazó la aprobación.", "L'accord a été refusé."]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_text_is_translated() {
        let sources = [include_str!("gui.rs"), include_str!("bin/quarz.rs")];
        let known: std::collections::HashSet<&str> = TABLE.iter().map(|(key, _)| *key).collect();
        let mut missing = Vec::new();
        for source in sources {
            for marker in ["tr(\"", "trf(\""] {
                let mut rest = source;
                while let Some(at) = rest.find(marker) {
                    rest = &rest[at + marker.len()..];
                    let end = rest.find('"').unwrap();
                    let key = rest[..end].replace("\\n", "\n");
                    if !known.contains(key.as_str()) && !missing.contains(&key) {
                        missing.push(key);
                    }
                }
            }
        }
        assert!(missing.is_empty(), "nicht übersetzt: {missing:#?}");
    }

    #[test]
    fn placeholders_survive_translation() {
        for (german, others) in TABLE {
            let names = |text: &str| -> Vec<String> {
                let mut found: Vec<String> = text.split('{').skip(1).filter_map(|p| p.split_once('}')).map(|(n, _)| n.to_string()).collect();
                found.sort();
                found
            };
            for other in others {
                assert_eq!(names(german), names(other), "Platzhalter in: {german}");
            }
        }
    }
}
