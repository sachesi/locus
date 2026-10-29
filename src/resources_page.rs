use crate::format;
use crate::gpu::Gpu;
use crate::graph::Graph;
use crate::sampler::System;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::sync::Arc;

struct CoreView {
    graph: Graph,
    value: gtk::Label,
}

struct GpuView {
    slot: Arc<str>,
    graph: Graph,
    legend: adw::WrapBox,
    usage: gtk::Label,
    memory_legend: gtk::Box,
    memory: gtk::Label,
    temperature_legend: gtk::Box,
    temperature: gtk::Label,
    unreported: gtk::Label,
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate, glib::Properties)]
    #[template(resource = "/io/github/sachesi/locus/ui/resources_page.ui")]
    #[properties(wrapper_type = super::ResourcesPage)]
    pub struct ResourcesPage {
        #[template_child]
        pub cpu_model: TemplateChild<gtk::Label>,
        #[template_child]
        pub cpu_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub cpu_graph: TemplateChild<Graph>,
        #[template_child]
        pub cores: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub cards: TemplateChild<gtk::Box>,
        #[template_child]
        pub memory_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub memory_graph: TemplateChild<Graph>,
        #[template_child]
        pub memory_detail: TemplateChild<gtk::Label>,
        #[template_child]
        pub swap_legend: TemplateChild<gtk::Box>,
        #[template_child]
        pub swap_detail: TemplateChild<gtk::Label>,
        #[template_child]
        pub network_graph: TemplateChild<Graph>,
        #[template_child]
        pub received: TemplateChild<gtk::Label>,
        #[template_child]
        pub sent: TemplateChild<gtk::Label>,
        #[template_child]
        pub disk_graph: TemplateChild<Graph>,
        #[template_child]
        pub read: TemplateChild<gtk::Label>,
        #[template_child]
        pub written: TemplateChild<gtk::Label>,
        #[template_child]
        pub gpus: TemplateChild<gtk::Box>,

        /// Set by the window's breakpoints; stacks the memory, network and disk cards
        /// rather than setting them side by side.
        #[property(get, set = Self::set_compact)]
        compact: Cell<bool>,

        pub(super) core_views: RefCell<Vec<CoreView>>,
        pub(super) gpu_views: RefCell<Vec<GpuView>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ResourcesPage {
        const NAME: &'static str = "LocusResourcesPage";
        type Type = super::ResourcesPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Graph::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for ResourcesPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.network_graph.set_scale_label(format::rate);
            self.disk_graph.set_scale_label(format::rate);
        }
    }

    impl WidgetImpl for ResourcesPage {}
    impl BinImpl for ResourcesPage {}

    impl ResourcesPage {
        fn set_compact(&self, compact: bool) {
            self.compact.set(compact);
            let orientation = if compact {
                gtk::Orientation::Vertical
            } else {
                gtk::Orientation::Horizontal
            };
            self.cards.set_orientation(orientation);
            self.gpus.set_orientation(orientation);
        }
    }
}

glib::wrapper! {
    pub struct ResourcesPage(ObjectSubclass<imp::ResourcesPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ResourcesPage {
    /// Adds the reading to the graphs, and writes it out beside them when the page is
    /// `shown`; a page off screen is written out by the reading taken when it comes back.
    pub fn apply(&self, system: &System, shown: bool) {
        let imp = self.imp();
        let memory = &system.memory;
        let memory_share = share(memory.used, memory.total);
        imp.cpu_graph.push(&[system.cpu]);
        if memory.swap_total > 0 {
            imp.memory_graph
                .push(&[memory_share, share(memory.swap_used, memory.swap_total)]);
        } else {
            imp.memory_graph.push(&[memory_share]);
        }
        imp.network_graph.push(&[system.received, system.sent]);
        imp.disk_graph.push(&[system.read, system.written]);
        self.apply_cores(&system.cores, shown);
        self.apply_gpus(&system.gpus, shown);
        if !shown {
            return;
        }

        if let Some(model) = &system.cpu_model
            && imp.cpu_model.label() != **model
        {
            imp.cpu_model.set_label(model);
            imp.cpu_model.set_visible(true);
        }
        imp.cpu_value.set_label(&format::percent(system.cpu, 0));
        imp.memory_value
            .set_label(&format::percent(memory_share, 0));
        imp.memory_detail
            .set_label(&format::used_of(memory.used, memory.total));
        imp.swap_legend.set_visible(memory.swap_total > 0);
        if memory.swap_total > 0 {
            imp.swap_detail
                .set_label(&format::used_of(memory.swap_used, memory.swap_total));
        }
        imp.received.set_label(&format::rate(system.received));
        imp.sent.set_label(&format::rate(system.sent));
        imp.read.set_label(&format::rate(system.read));
        imp.written.set_label(&format::rate(system.written));
    }

    fn apply_gpus(&self, gpus: &[Gpu], shown: bool) {
        let imp = self.imp();
        let mut views = imp.gpu_views.borrow_mut();
        let same = views.len() == gpus.len()
            && views
                .iter()
                .zip(gpus)
                .all(|(view, gpu)| view.slot == gpu.slot);
        if !same {
            while let Some(child) = imp.gpus.first_child() {
                imp.gpus.remove(&child);
            }
            views.clear();
            for (index, gpu) in gpus.iter().enumerate() {
                let number = (gpus.len() > 1).then_some(index + 1);
                let (view, widget) = gpu_view(gpu, number);
                imp.gpus.append(&widget);
                views.push(view);
            }
            imp.gpus.set_visible(!gpus.is_empty());
        }
        for (view, gpu) in views.iter().zip(gpus) {
            let memory_share = gpu.memory.map(|(used, total)| share(used, total));
            // Once shown, the graph stays through a reading the driver fails.
            let reported = gpu.usage.is_some() || memory_share.is_some();
            if reported {
                view.graph.set_visible(true);
                view.legend.set_visible(true);
                view.unreported.set_visible(false);
            }
            let usage = gpu.usage.unwrap_or(0.0);
            match memory_share {
                Some(memory_share) => view.graph.push(&[usage, memory_share]),
                None => view.graph.push(&[usage]),
            }
            if !shown {
                continue;
            }
            view.usage.set_label(&format::percent(usage, 0));
            view.memory_legend.set_visible(gpu.memory.is_some());
            if let Some((used, total)) = gpu.memory {
                view.memory.set_label(&format::used_of(used, total));
            }
            view.temperature_legend
                .set_visible(gpu.temperature.is_some());
            if let Some(temperature) = gpu.temperature {
                view.temperature
                    .set_label(&format::temperature(temperature));
            }
        }
    }

    fn apply_cores(&self, cores: &[f64], shown: bool) {
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
            // One processor is the whole machine, already drawn above.
            imp.cores.set_visible(cores.len() > 1);
        }
        for (view, &usage) in views.iter().zip(cores) {
            view.graph.push(&[usage]);
            if shown {
                view.value.set_label(&format::percent(usage, 0));
            }
        }
    }
}

fn share(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        used as f64 * 100.0 / total as f64
    }
}

/// `number` tells the cards apart where there are several.
fn gpu_view(gpu: &Gpu, number: Option<usize>) -> (GpuView, gtk::Widget) {
    let title = match number {
        // Translators: one of several graphics cards; {number} counts from 1.
        Some(number) => gettext("GPU {number}").replace("{number}", &number.to_string()),
        None => gettext("GPU"),
    };
    let title = gtk::Label::builder()
        .label(&title)
        .xalign(0.0)
        .css_classes(["title-4"])
        .build();
    let name = gtk::Label::builder()
        .label(gpu.name.as_deref().unwrap_or_default())
        .visible(gpu.name.is_some())
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["caption", "dim-label"])
        .build();
    let names = gtk::Box::new(gtk::Orientation::Vertical, 0);
    names.append(&title);
    names.append(&name);

    let graph = Graph::default();
    graph.set_maximum(100.0);
    graph.set_height_request(100);
    graph.set_visible(false);
    graph.update_property(&[gtk::accessible::Property::Label(&gettext(
        "GPU usage and video memory in use over the last readings",
    ))]);

    let (usage_legend, usage) = legend_item(Some("first"), &gettext("Usage"));
    let (memory_legend, memory) = legend_item(Some("second"), &gettext("Video Memory"));
    let (temperature_legend, temperature) = legend_item(None, &gettext("Temperature"));
    let legend = adw::WrapBox::builder()
        .child_spacing(24)
        .line_spacing(6)
        .visible(false)
        .build();
    legend.append(&usage_legend);
    legend.append(&memory_legend);
    legend.append(&temperature_legend);

    let unreported = gtk::Label::builder()
        .label(gettext("The driver does not report usage"))
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label"])
        .build();

    let widget = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .css_classes(["card", "resource"])
        .build();
    widget.append(&names);
    widget.append(&graph);
    widget.append(&legend);
    widget.append(&unreported);
    (
        GpuView {
            slot: gpu.slot.clone(),
            graph,
            legend,
            usage,
            memory_legend,
            memory,
            temperature_legend,
            temperature,
            unreported,
        },
        widget.upcast(),
    )
}

/// A line of a legend, with the swatch of the series it names, if it names one.
fn legend_item(swatch: Option<&str>, label: &str) -> (gtk::Box, gtk::Label) {
    let item = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    if let Some(swatch) = swatch {
        item.append(
            &gtk::Box::builder()
                .valign(gtk::Align::Center)
                .css_classes(["swatch", swatch])
                .build(),
        );
    }
    item.append(
        &gtk::Label::builder()
            .label(label)
            .css_classes(["dim-label"])
            .build(),
    );
    let value = gtk::Label::builder().css_classes(["numeric"]).build();
    item.append(&value);
    (item, value)
}

fn core_view(number: usize) -> (CoreView, gtk::Widget) {
    let graph = Graph::default();
    graph.set_maximum(100.0);
    graph.set_plain(true);
    graph.set_height_request(36);
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
