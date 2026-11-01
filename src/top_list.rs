use crate::heat_cell::HeatCell;
use crate::sampler::ProcessSample;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

/// How many apps and processes the list shows.
const SHOWN: usize = 3;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct TopList {
        pub list: gtk::ListBox,
        pub(super) rows: RefCell<Vec<(gtk::Label, HeatCell)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TopList {
        const NAME: &'static str = "LocusTopList";
        type Type = super::TopList;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for TopList {
        fn constructed(&self) {
            self.parent_constructed();
            self.list.set_selection_mode(gtk::SelectionMode::None);
            self.list.add_css_class("boxed-list");
            self.list.set_valign(gtk::Align::Start);
            self.list
                .update_property(&[gtk::accessible::Property::Label(&gettextrs::gettext(
                    "What uses the most",
                ))]);
            let mut rows = self.rows.borrow_mut();
            for _ in 0..SHOWN {
                let name = gtk::Label::builder()
                    .xalign(0.0)
                    .hexpand(true)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .build();
                let value = HeatCell::default();
                value.set_rounded(true);
                value.set_valign(gtk::Align::Center);
                value.set_width_request(72);
                value.text().add_css_class("top-value");
                let content = gtk::Box::builder()
                    .spacing(12)
                    .margin_top(9)
                    .margin_bottom(9)
                    .margin_start(12)
                    .margin_end(9)
                    .build();
                content.append(&name);
                content.append(&value);
                self.list.append(
                    &gtk::ListBoxRow::builder()
                        .child(&content)
                        .activatable(false)
                        .visible(false)
                        .build(),
                );
                rows.push((name, value));
            }
            self.obj().set_child(Some(&self.list));
        }
    }

    impl WidgetImpl for TopList {}
    impl BinImpl for TopList {}
}

glib::wrapper! {
    /// The apps and processes using the most of something.
    pub struct TopList(ObjectSubclass<imp::TopList>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TopList {
    /// Lists the largest of `value` over `samples`, the processes of an app counted as one.
    /// `text` and `level` say how a value is written and tinted.
    pub fn show(
        &self,
        samples: &[ProcessSample],
        value: impl Fn(&ProcessSample) -> f64,
        text: impl Fn(f64) -> String,
        level: impl Fn(f64) -> u32,
    ) {
        let mut totals: HashMap<Arc<str>, f64> = HashMap::new();
        for sample in samples {
            let name = sample.app.as_ref().map_or(&sample.name, |app| &app.name);
            *totals.entry(name.clone()).or_insert(0.0) += value(sample);
        }
        let mut totals: Vec<_> = totals.into_iter().filter(|top| top.1 > 0.0).collect();
        totals.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let rows = self.imp().rows.borrow();
        for (index, (name, cell)) in rows.iter().enumerate() {
            let top = totals.get(index);
            if let Some(row) = name.ancestor(gtk::ListBoxRow::static_type()) {
                row.set_visible(top.is_some());
            }
            if let Some((top_name, amount)) = top {
                name.set_label(top_name);
                cell.text().set_text(Some(&text(*amount)));
                cell.set_level(level(*amount));
            }
        }
    }
}
