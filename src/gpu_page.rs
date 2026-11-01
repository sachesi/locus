use crate::format;
use crate::gpu::Gpu;
use crate::graph::Graph;
use crate::heat_cell;
use crate::sampler::ProcessSample;
use crate::stat::Stat;
use crate::top_list::TopList;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use std::cell::RefCell;

struct EngineView {
    name: String,
    graph: Graph,
    value: gtk::Label,
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/locus/ui/gpu_page.ui")]
    pub struct GpuPage {
        #[template_child]
        pub title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub usage_card: TemplateChild<gtk::Box>,
        #[template_child]
        pub usage_graph: TemplateChild<Graph>,
        #[template_child]
        pub engines_card: TemplateChild<gtk::Box>,
        #[template_child]
        pub engines: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub memory_card: TemplateChild<gtk::Box>,
        #[template_child]
        pub memory_graph: TemplateChild<Graph>,
        #[template_child]
        pub note: TemplateChild<gtk::Label>,
        #[template_child]
        pub usage: TemplateChild<Stat>,
        #[template_child]
        pub memory: TemplateChild<Stat>,
        #[template_child]
        pub temperature: TemplateChild<Stat>,
        #[template_child]
        pub power: TemplateChild<Stat>,
        #[template_child]
        pub clock: TemplateChild<Stat>,
        #[template_child]
        pub fan: TemplateChild<Stat>,
        #[template_child]
        pub top: TemplateChild<TopList>,

        pub(super) engine_views: RefCell<Vec<EngineView>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GpuPage {
        const NAME: &'static str = "LocusGpuPage";
        type Type = super::GpuPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Graph::ensure_type();
            Stat::ensure_type();
            TopList::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for GpuPage {}
    impl WidgetImpl for GpuPage {}
    impl BinImpl for GpuPage {}
}

glib::wrapper! {
    pub struct GpuPage(ObjectSubclass<imp::GpuPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for GpuPage {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl GpuPage {
    pub fn set_name(&self, title: &str, gpu: &Gpu) {
        let imp = self.imp();
        imp.title.set_title(title);
        imp.title
            .set_subtitle(gpu.name.as_deref().unwrap_or_default());
    }

    pub fn apply(&self, gpu: &Gpu, shown: bool, unmeasured: bool) {
        let imp = self.imp();
        let memory_share = gpu.memory.map(|(used, total)| format::share(used, total));
        if !unmeasured {
            if let Some(usage) = gpu.usage {
                imp.usage_graph.push(&[usage]);
            }
            if let Some(share) = memory_share {
                imp.memory_graph.push(&[share]);
            }
        }
        if !shown {
            return;
        }
        imp.usage_card
            .set_visible(gpu.usage.is_some() && !gpu.asleep);
        imp.memory_card.set_visible(gpu.memory.is_some());
        let note = if gpu.asleep {
            Some(gettext(
                "The card is asleep, and is left so: reading it would wake it",
            ))
        } else if gpu.usage.is_none() && gpu.memory.is_none() {
            Some(gettext("The driver does not report usage"))
        } else {
            None
        };
        imp.note.set_visible(note.is_some());
        imp.note.set_label(note.as_deref().unwrap_or_default());
        imp.usage.show_value(
            gpu.usage
                .filter(|_| !gpu.asleep)
                .map(|usage| format::percent(usage, 0)),
        );
        imp.memory
            .show_value(gpu.memory.map(|(used, total)| format::used_of(used, total)));
        imp.temperature
            .show_value(gpu.temperature.map(format::temperature));
        imp.power.show_value(gpu.power.map(format::watts));
        imp.clock
            .show_value(gpu.clock.filter(|hertz| *hertz > 0.0).map(format::speed));
        imp.fan.show_value(gpu.fan.map(format::rpm));
        self.apply_engines(gpu);
    }

    /// Lists the apps and processes using this card the most. A process's use is not split
    /// by card, so where there are several this reads across them.
    pub fn show_top(&self, samples: &[ProcessSample]) {
        self.imp().top.show(
            samples,
            |sample| sample.gpu.unwrap_or(0.0),
            |share| format::percent(share, 1),
            heat_cell::gpu_level,
        );
    }

    fn apply_engines(&self, gpu: &Gpu) {
        let imp = self.imp();
        let mut views = imp.engine_views.borrow_mut();
        for (name, _) in &gpu.engines {
            if !views.iter().any(|view| view.name == *name) {
                let (view, widget) = engine_view(name);
                imp.engines.append(&widget);
                views.push(view);
            }
        }
        imp.engines_card.set_visible(!views.is_empty());
        for view in views.iter() {
            let share = gpu
                .engines
                .iter()
                .find(|(name, _)| *name == view.name)
                .map_or(0.0, |engine| engine.1);
            view.graph.push(&[share]);
            view.value.set_label(&format::percent(share, 0));
        }
    }
}

/// What the drivers call an engine, in words; a kind not known here keeps the driver's name.
fn engine_title(name: &str) -> String {
    match name {
        "gfx" | "render" => gettext("Graphics"),
        "compute" => gettext("Compute"),
        "dec" | "video" => gettext("Video Decode"),
        "enc" => gettext("Video Encode"),
        "copy" => gettext("Copy"),
        other => other.to_owned(),
    }
}

fn engine_view(name: &str) -> (EngineView, gtk::Widget) {
    let graph = Graph::default();
    graph.set_maximum(100.0);
    graph.set_height_request(96);
    let title = engine_title(name);
    graph.update_property(&[gtk::accessible::Property::Label(&title)]);
    let label = gtk::Label::builder()
        .label(&title)
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
    widget.append(&caption);
    widget.append(&graph);
    let child = gtk::FlowBoxChild::builder()
        .child(&widget)
        .focusable(false)
        .build();
    (
        EngineView {
            name: name.to_owned(),
            graph,
            value,
        },
        child.upcast(),
    )
}
