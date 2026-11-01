use crate::format;
use crate::graph::Graph;
use crate::heat_cell::{self, HeatCell};
use crate::range_switch::RangeSwitch;
use crate::sampler::{ProcessSample, System};
use crate::stat::Stat;
use crate::top_list::TopList;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use std::cell::RefCell;

struct CoreView {
    graph: Graph,
    value: gtk::Label,
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/locus/ui/cpu_page.ui")]
    pub struct CpuPage {
        #[template_child]
        pub title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub graph: TemplateChild<Graph>,
        #[template_child]
        pub cores: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub usage: TemplateChild<Stat>,
        #[template_child]
        pub speed: TemplateChild<Stat>,
        #[template_child]
        pub temperature: TemplateChild<Stat>,
        #[template_child]
        pub load: TemplateChild<Stat>,
        #[template_child]
        pub processes: TemplateChild<Stat>,
        #[template_child]
        pub threads: TemplateChild<Stat>,
        #[template_child]
        pub pressure: TemplateChild<Stat>,
        #[template_child]
        pub uptime: TemplateChild<Stat>,
        #[template_child]
        pub top: TemplateChild<TopList>,

        pub(super) core_views: RefCell<Vec<CoreView>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CpuPage {
        const NAME: &'static str = "LocusCpuPage";
        type Type = super::CpuPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Graph::ensure_type();
            RangeSwitch::ensure_type();
            HeatCell::ensure_type();
            Stat::ensure_type();
            TopList::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for CpuPage {}
    impl WidgetImpl for CpuPage {}
    impl BinImpl for CpuPage {}
}

glib::wrapper! {
    pub struct CpuPage(ObjectSubclass<imp::CpuPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl CpuPage {
    /// Adds the reading to the graphs, and writes it out when the page is `shown`. A reading
    /// `unmeasured` has no rates, and leaves the graphs and the rates alone.
    pub fn apply(&self, system: &System, shown: bool, unmeasured: bool) {
        let imp = self.imp();
        if !unmeasured {
            imp.graph.push(&[system.cpu]);
        }
        self.apply_cores(&system.cores, !unmeasured, shown && !unmeasured);
        if !shown {
            return;
        }
        if let Some(model) = &system.cpu_model {
            imp.title.set_subtitle(model);
        }
        if !unmeasured {
            imp.usage.show_value(Some(format::percent(system.cpu, 0)));
        }
        let Some(detail) = &system.cpu_detail else {
            return;
        };
        imp.speed.show_value(detail.speed.map(format::speed));
        imp.temperature
            .show_value(detail.temperature.map(format::temperature));
        imp.load
            .show_value(detail.load.map(|load| format::number(load, 2)));
        imp.processes.show_value(Some(detail.processes.to_string()));
        imp.threads
            .show_value(detail.threads.map(|threads| threads.to_string()));
        imp.pressure
            .show_value(detail.pressure.map(|share| format::percent(share, 1)));
        imp.uptime.show_value(detail.uptime.map(format::uptime));
    }

    /// Lists the apps and processes using the most processor time.
    pub fn show_top(&self, samples: &[ProcessSample]) {
        self.imp().top.show(
            samples,
            |sample| sample.cpu,
            |share| format::percent(share, 1),
            heat_cell::cpu_level,
        );
    }

    fn apply_cores(&self, cores: &[f64], graph: bool, shown: bool) {
        let imp = self.imp();
        let mut views = imp.core_views.borrow_mut();
        if views.len() != cores.len() {
            imp.cores.remove_all();
            views.clear();
            for number in 1..=cores.len() {
                let (view, widget) = core_view(number);
                imp.cores.append(&widget);
                views.push(view);
            }
        }
        for (view, &usage) in views.iter().zip(cores) {
            if graph {
                view.graph.push(&[usage]);
            }
            if shown {
                view.value.set_label(&format::percent(usage, 0));
            }
        }
    }
}

fn core_view(number: usize) -> (CoreView, gtk::Widget) {
    let graph = Graph::default();
    graph.set_maximum(100.0);
    graph.set_plain(true);
    graph.set_tint("blue");
    graph.set_height_request(48);
    // Translators: one core of the processor; {number} counts from 1.
    let name = gettext("CPU {number}").replace("{number}", &number.to_string());
    graph.update_property(&[gtk::accessible::Property::Label(&name)]);
    let label = gtk::Label::builder()
        .label(&name)
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["caption", "dim-label"])
        .build();
    let value = gtk::Label::builder()
        .css_classes(["caption", "numeric"])
        .build();
    let caption = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    caption.append(&label);
    caption.append(&value);
    let widget = gtk::Box::new(gtk::Orientation::Vertical, 4);
    widget.append(&graph);
    widget.append(&caption);
    let child = gtk::FlowBoxChild::builder()
        .child(&widget)
        .focusable(false)
        .build();
    (CoreView { graph, value }, child.upcast())
}
