use crate::format;
use crate::sampler::FileSystem;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use std::cell::RefCell;

struct FileSystemView {
    mount_point: String,
    free: gtk::Label,
    bar: gtk::LevelBar,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct FileSystemList {
        pub list: gtk::ListBox,
        pub(super) views: RefCell<Vec<FileSystemView>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FileSystemList {
        const NAME: &'static str = "LocusFileSystemList";
        type Type = super::FileSystemList;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for FileSystemList {
        fn constructed(&self) {
            self.parent_constructed();
            self.list.set_selection_mode(gtk::SelectionMode::None);
            self.list.set_valign(gtk::Align::Start);
            self.list.add_css_class("boxed-list");
            self.obj().set_child(Some(&self.list));
        }
    }

    impl WidgetImpl for FileSystemList {}
    impl BinImpl for FileSystemList {}
}

glib::wrapper! {
    /// Mounted file systems and the room left on each.
    pub struct FileSystemList(ObjectSubclass<imp::FileSystemList>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl FileSystemList {
    pub fn apply(&self, filesystems: &[&FileSystem]) {
        let imp = self.imp();
        let mut views = imp.views.borrow_mut();
        let same = views.len() == filesystems.len()
            && views
                .iter()
                .zip(filesystems)
                .all(|(view, fs)| view.mount_point == fs.mount_point);
        if !same {
            imp.list.remove_all();
            views.clear();
            for filesystem in filesystems {
                let (view, row) = filesystem_row(filesystem);
                imp.list.append(&row);
                views.push(view);
            }
        }
        for (view, filesystem) in views.iter().zip(filesystems) {
            view.free
                .set_label(&format::free_of(filesystem.available, filesystem.size));
            view.bar
                .set_value(filesystem.used as f64 / filesystem.size as f64);
        }
    }
}

fn filesystem_row(filesystem: &FileSystem) -> (FileSystemView, gtk::ListBoxRow) {
    let name = gtk::Label::builder()
        .label(&filesystem.mount_point)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    let device = gtk::Label::builder()
        .label(format!("{} · {}", filesystem.fs_type, filesystem.source))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .css_classes(["caption", "dim-label"])
        .build();
    let names = gtk::Box::new(gtk::Orientation::Vertical, 0);
    names.set_hexpand(true);
    names.append(&name);
    names.append(&device);
    let free = gtk::Label::builder()
        .valign(gtk::Align::End)
        .css_classes(["dim-label", "numeric"])
        .build();
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    top.append(&names);
    top.append(&free);
    let bar = gtk::LevelBar::new();
    // The default offsets colour a nearly empty bar as a warning, as for a battery; here
    // the bar turns to one past nine tenths full.
    for offset in [
        gtk::LEVEL_BAR_OFFSET_LOW,
        gtk::LEVEL_BAR_OFFSET_HIGH,
        gtk::LEVEL_BAR_OFFSET_FULL,
    ] {
        bar.remove_offset_value(Some(offset));
    }
    bar.add_offset_value("roomy", 0.9);
    bar.add_offset_value("almost-full", 1.0);
    bar.update_property(&[gtk::accessible::Property::Label(&filesystem.mount_point)]);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&top);
    content.append(&bar);
    let row = gtk::ListBoxRow::builder()
        .child(&content)
        .activatable(false)
        .build();
    (
        FileSystemView {
            mount_point: filesystem.mount_point.clone(),
            free,
            bar,
        },
        row,
    )
}
