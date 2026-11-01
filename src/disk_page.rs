use crate::file_system_list::FileSystemList;
use crate::format;
use crate::graph::Graph;
use crate::range_switch::RangeSwitch;
use crate::sampler::{Disk, FileSystem};
use crate::stat::Stat;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/locus/ui/disk_page.ui")]
    pub struct DiskPage {
        #[template_child]
        pub title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub active_graph: TemplateChild<Graph>,
        #[template_child]
        pub transfer_graph: TemplateChild<Graph>,
        #[template_child]
        pub active: TemplateChild<Stat>,
        #[template_child]
        pub read: TemplateChild<Stat>,
        #[template_child]
        pub written: TemplateChild<Stat>,
        #[template_child]
        pub response: TemplateChild<Stat>,
        #[template_child]
        pub temperature: TemplateChild<Stat>,
        #[template_child]
        pub own_group: TemplateChild<gtk::Box>,
        #[template_child]
        pub own_filesystems: TemplateChild<FileSystemList>,
        #[template_child]
        pub other_group: TemplateChild<gtk::Box>,
        #[template_child]
        pub other_filesystems: TemplateChild<FileSystemList>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DiskPage {
        const NAME: &'static str = "LocusDiskPage";
        type Type = super::DiskPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            FileSystemList::ensure_type();
            Graph::ensure_type();
            RangeSwitch::ensure_type();
            Stat::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DiskPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.transfer_graph.set_scale_label(format::rate);
        }
    }

    impl WidgetImpl for DiskPage {}
    impl BinImpl for DiskPage {}
}

glib::wrapper! {
    pub struct DiskPage(ObjectSubclass<imp::DiskPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for DiskPage {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl DiskPage {
    pub fn set_name(&self, title: &str, disk: &Disk) {
        let imp = self.imp();
        imp.title.set_title(title);
        let size = format::size(disk.size);
        imp.title.set_subtitle(&match &disk.model {
            Some(model) => format!("{model}, {size}"),
            None => size,
        });
    }

    pub fn apply(&self, disk: &Disk, shown: bool, unmeasured: bool) {
        let imp = self.imp();
        if !unmeasured {
            imp.active_graph.push(&[disk.active]);
            imp.transfer_graph.push(&[disk.read, disk.written]);
        }
        if !shown || unmeasured {
            return;
        }
        imp.active.set_value(format::percent(disk.active, 0));
        imp.read.set_value(format::rate(disk.read));
        imp.written.set_value(format::rate(disk.written));
        imp.response
            .show_value(disk.response.map(format::milliseconds));
        imp.temperature
            .show_value(disk.temperature.map(format::temperature));
    }

    /// Lists the file systems on this disk; the `first` disk's page also lists those on no
    /// disk, so that they are somewhere.
    pub fn show_filesystems(&self, name: &str, first: bool, filesystems: &[FileSystem]) {
        let imp = self.imp();
        let own: Vec<_> = filesystems
            .iter()
            .filter(|fs| fs.disks.iter().any(|disk| &**disk == name))
            .collect();
        let other: Vec<_> = filesystems
            .iter()
            .filter(|fs| first && fs.disks.is_empty())
            .collect();
        imp.own_group.set_visible(!own.is_empty());
        imp.own_filesystems.apply(&own);
        imp.other_group.set_visible(!other.is_empty());
        imp.other_filesystems.apply(&other);
    }
}
