use crate::format;
use crate::sampler::FileSystem;
use adw::subclass::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;

/// The widgets of one file system's row.
struct FileSystemView {
    mount_point: String,
    free: gtk::Label,
    bar: gtk::LevelBar,
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/locus/ui/filesystems_page.ui")]
    pub struct FileSystemsPage {
        #[template_child]
        pub filesystems: TemplateChild<gtk::ListBox>,

        pub(super) views: RefCell<Vec<FileSystemView>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FileSystemsPage {
        const NAME: &'static str = "LocusFileSystemsPage";
        type Type = super::FileSystemsPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for FileSystemsPage {}
    impl WidgetImpl for FileSystemsPage {}
    impl BinImpl for FileSystemsPage {}
}

glib::wrapper! {
    pub struct FileSystemsPage(ObjectSubclass<imp::FileSystemsPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl FileSystemsPage {
    pub fn apply(&self, filesystems: &[FileSystem]) {
        let imp = self.imp();
        let mut views = imp.views.borrow_mut();
        let same = views.len() == filesystems.len()
            && views
                .iter()
                .zip(filesystems)
                .all(|(view, fs)| view.mount_point == fs.mount_point);
        if !same {
            imp.filesystems.remove_all();
            views.clear();
            for filesystem in filesystems {
                let (view, row) = filesystem_row(filesystem);
                imp.filesystems.append(&row);
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
