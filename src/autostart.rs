//! The apps that start with the session, as desktop entries in the XDG autostart folders. An
//! entry in the user's folder stands in for one of the same name in a system folder, so that
//! is where any change is written; system folders are only read.

use gtk::gio;
use gtk::glib;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const GROUP: &str = "Desktop Entry";

/// An app that can start with the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The file's name without `.desktop`, which names it across the folders.
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub enabled: bool,
    /// Whether the user's folder has it, so it can be changed without a copy.
    pub in_user_folder: bool,
    /// Whether a system folder has it too, so removing the user's copy would not remove it.
    pub in_system_folder: bool,
    path: PathBuf,
}

fn user_folder() -> PathBuf {
    glib::user_config_dir().join("autostart")
}

/// The system folders, the least preferred first, as later ones take over from earlier ones.
fn system_folders() -> Vec<PathBuf> {
    glib::system_config_dirs()
        .iter()
        .rev()
        .map(|folder| folder.join("autostart"))
        .collect()
}

/// The apps that start with this session, by name.
pub fn list() -> Vec<Entry> {
    let desktops: Vec<String> = glib::getenv("XDG_CURRENT_DESKTOP")
        .and_then(|desktops| desktops.into_string().ok())
        .map(|desktops| desktops.split(':').map(str::to_owned).collect())
        .unwrap_or_default();
    let mut entries = gather(&system_folders(), &user_folder(), &desktops);
    entries.sort_by_key(|entry| entry.name.to_lowercase());
    entries
}

fn gather(system: &[PathBuf], user: &Path, desktops: &[String]) -> Vec<Entry> {
    let mut found: BTreeMap<String, Entry> = BTreeMap::new();
    for folder in system {
        read_folder(folder, false, desktops, &mut found);
    }
    read_folder(user, true, desktops, &mut found);
    found.into_values().collect()
}

fn read_folder(
    folder: &Path,
    is_user: bool,
    desktops: &[String],
    found: &mut BTreeMap<String, Entry>,
) {
    let Ok(files) = fs::read_dir(folder) else {
        return;
    };
    for file in files.flatten() {
        let path = file.path();
        if path
            .extension()
            .is_none_or(|extension| extension != "desktop")
        {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let in_system_folder =
            !is_user || found.get(id).is_some_and(|entry| entry.in_system_folder);
        match read_entry(&path, id, desktops) {
            Some(mut entry) => {
                entry.in_user_folder = is_user;
                entry.in_system_folder = in_system_folder;
                found.insert(id.to_owned(), entry);
            }
            None => {
                found.remove(id);
            }
        }
    }
}

/// `None` where the file is no app for this desktop, which also takes it from the list where
/// it stands in for one in a system folder.
fn read_entry(path: &Path, id: &str, desktops: &[String]) -> Option<Entry> {
    let file = glib::KeyFile::new();
    file.load_from_file(path, glib::KeyFileFlags::NONE).ok()?;
    let list = |key: &str| {
        file.string_list(GROUP, key)
            .map(|items| items.iter().map(ToString::to_string).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    if !applies_to(&list("OnlyShowIn"), &list("NotShowIn"), desktops) {
        return None;
    }
    let flag = |key: &str| file.boolean(GROUP, key).ok();
    let name = file
        .locale_string(GROUP, "Name", None)
        .map_or_else(|_| id.to_owned(), String::from);
    Some(Entry {
        id: id.to_owned(),
        name,
        icon: file.string(GROUP, "Icon").ok().map(String::from),
        enabled: flag("Hidden") != Some(true) && flag("X-GNOME-Autostart-enabled") != Some(false),
        in_user_folder: false,
        in_system_folder: false,
        path: path.to_owned(),
    })
}

fn applies_to(only: &[String], not: &[String], desktops: &[String]) -> bool {
    let named = |list: &[String]| desktops.iter().any(|desktop| list.contains(desktop));
    !named(not) && (only.is_empty() || named(only))
}

/// Has the app start with the session, or not, in the user's folder.
pub fn set_enabled(entry: &Entry, enabled: bool) -> Result<(), glib::Error> {
    set_enabled_in(entry, enabled, &user_folder())
}

fn set_enabled_in(entry: &Entry, enabled: bool, user: &Path) -> Result<(), glib::Error> {
    let file = glib::KeyFile::new();
    file.load_from_file(&entry.path, glib::KeyFileFlags::KEEP_COMMENTS)?;
    file.set_boolean(GROUP, "Hidden", !enabled);
    if enabled
        && file
            .has_key(GROUP, "X-GNOME-Autostart-enabled")
            .unwrap_or(false)
    {
        file.set_boolean(GROUP, "X-GNOME-Autostart-enabled", true);
    }
    save(&file, user, &entry.id)
}

/// Has an installed app start with the session, by the name of its desktop file.
pub fn add(desktop_id: &str) -> Result<(), glib::Error> {
    let mut folders = vec![glib::user_data_dir()];
    folders.extend(glib::system_data_dirs());
    let source = folders
        .iter()
        .map(|folder| folder.join("applications").join(desktop_id))
        .find(|path| path.is_file());
    let Some(source) = source else {
        let message = format!("no desktop file for {desktop_id}");
        return Err(glib::Error::new(gio::IOErrorEnum::NotFound, &message));
    };
    let file = glib::KeyFile::new();
    file.load_from_file(&source, glib::KeyFileFlags::KEEP_COMMENTS)?;
    save(
        &file,
        &user_folder(),
        desktop_id.trim_end_matches(".desktop"),
    )
}

fn save(file: &glib::KeyFile, folder: &Path, id: &str) -> Result<(), glib::Error> {
    fs::create_dir_all(folder)
        .map_err(|error| glib::Error::new(gio::IOErrorEnum::Failed, &error.to_string()))?;
    file.save_to_file(folder.join(format!("{id}.desktop")))
}

/// Takes the app from the user's folder.
pub fn remove(entry: &Entry) -> Result<(), glib::Error> {
    fs::remove_file(&entry.path)
        .map_err(|error| glib::Error::new(gio::IOErrorEnum::Failed, &error.to_string()))
}

/// The service systemd makes of an entry where the session is started by it; a dash in the id
/// is escaped, like any character a unit's instance name cannot hold.
pub fn unit_of(id: &str) -> String {
    let mut escaped = String::new();
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':') {
            escaped.push(char::from(byte));
        } else {
            escaped.push_str(&format!("\\x{byte:02x}"));
        }
    }
    format!("app-{escaped}@autostart.service")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(folder: &Path, id: &str, lines: &str) {
        fs::create_dir_all(folder).unwrap();
        let text = format!("[Desktop Entry]\nType=Application\nExec=true\n{lines}\n");
        fs::write(folder.join(format!("{id}.desktop")), text).unwrap();
    }

    fn folders() -> (PathBuf, PathBuf, PathBuf) {
        let root = glib::mkdtemp(glib::tmp_dir().join("locus-autostart-XXXXXX")).unwrap();
        (root.join("system"), root.join("user"), root)
    }

    #[test]
    fn the_users_folder_stands_in_for_a_system_one() {
        let (system, user, root) = folders();
        write(&system, "tray", "Name=Tray");
        write(&system, "agent", "Name=Agent");
        write(&user, "tray", "Name=Tray\nHidden=true");
        write(&user, "mine", "Name=Mine");
        let entries = gather(&[system], &user, &[]);
        let tray = entries.iter().find(|entry| entry.id == "tray").unwrap();
        assert!(!tray.enabled && tray.in_user_folder && tray.in_system_folder);
        let agent = entries.iter().find(|entry| entry.id == "agent").unwrap();
        assert!(agent.enabled && !agent.in_user_folder && agent.in_system_folder);
        let mine = entries.iter().find(|entry| entry.id == "mine").unwrap();
        assert!(mine.in_user_folder && !mine.in_system_folder);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn turning_one_off_copies_it_and_leaves_the_system_folder_alone() {
        let (system, user, root) = folders();
        write(&system, "tray", "Name=Tray");
        let entries = gather(std::slice::from_ref(&system), &user, &[]);
        set_enabled_in(&entries[0], false, &user).unwrap();
        let entries = gather(std::slice::from_ref(&system), &user, &[]);
        assert!(!entries[0].enabled && entries[0].in_user_folder);
        assert!(
            !fs::read_to_string(system.join("tray.desktop"))
                .unwrap()
                .contains("Hidden")
        );
        set_enabled_in(&entries[0], true, &user).unwrap();
        assert!(gather(&[system], &user, &[])[0].enabled);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn entries_for_other_desktops_are_left_out() {
        let here = ["Here".to_owned()];
        let there = ["There".to_owned()];
        assert!(applies_to(&[], &[], &here));
        assert!(applies_to(&here, &[], &here));
        assert!(!applies_to(&there, &[], &here));
        assert!(!applies_to(&[], &here, &here));
    }

    #[test]
    fn unit_names_escape_what_an_instance_cannot_hold() {
        assert_eq!(
            unit_of("org.example.App"),
            "app-org.example.App@autostart.service"
        );
        assert_eq!(unit_of("key-ring"), "app-key\\x2dring@autostart.service");
    }
}
