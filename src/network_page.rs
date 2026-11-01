use crate::format;
use crate::graph::Graph;
use crate::range_switch::RangeSwitch;
use crate::sampler::{Link, LinkDetail};
use crate::stat::Stat;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/locus/ui/network_page.ui")]
    pub struct NetworkPage {
        #[template_child]
        pub title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub graph: TemplateChild<Graph>,
        #[template_child]
        pub received: TemplateChild<Stat>,
        #[template_child]
        pub sent: TemplateChild<Stat>,
        #[template_child]
        pub total_received: TemplateChild<Stat>,
        #[template_child]
        pub total_sent: TemplateChild<Stat>,
        #[template_child]
        pub speed: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub ipv4: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub ipv6: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub hardware_address: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub driver: TemplateChild<adw::ActionRow>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NetworkPage {
        const NAME: &'static str = "LocusNetworkPage";
        type Type = super::NetworkPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Graph::ensure_type();
            RangeSwitch::ensure_type();
            Stat::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for NetworkPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.graph.set_scale_label(format::rate);
        }
    }

    impl WidgetImpl for NetworkPage {}
    impl BinImpl for NetworkPage {}
}

glib::wrapper! {
    pub struct NetworkPage(ObjectSubclass<imp::NetworkPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for NetworkPage {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl NetworkPage {
    pub fn set_name(&self, title: &str, subtitle: &str) {
        let imp = self.imp();
        imp.title.set_title(title);
        imp.title.set_subtitle(subtitle);
    }

    pub fn apply(&self, link: &Link, shown: bool, unmeasured: bool) {
        let imp = self.imp();
        if !unmeasured {
            imp.graph.push(&[link.received, link.sent]);
        }
        if !shown {
            return;
        }
        imp.total_received
            .set_value(format::size(link.total_received));
        imp.total_sent.set_value(format::size(link.total_sent));
        if let Some(detail) = &link.detail {
            self.show_detail(detail);
        }
        if !unmeasured {
            imp.received.set_value(format::rate(link.received));
            imp.sent.set_value(format::rate(link.sent));
        }
    }

    fn show_detail(&self, detail: &LinkDetail) {
        let imp = self.imp();
        let speed = detail.speed.filter(|speed| *speed > 0).map(|speed| {
            if speed >= 1000 {
                // Translators: the speed of a network connection, such as "2.5 Gbit/s".
                gettext("{value} Gbit/s")
                    .replace("{value}", &format::number(f64::from(speed) / 1000.0, 1))
            } else {
                // Translators: the speed of a network connection, such as "100 Mbit/s".
                gettext("{value} Mbit/s").replace("{value}", &speed.to_string())
            }
        });
        let show = |row: &adw::ActionRow, value: Option<String>| {
            row.set_visible(value.is_some());
            if let Some(value) = value {
                row.set_subtitle(&value);
            }
        };
        let list = |addresses: &[String]| (!addresses.is_empty()).then(|| addresses.join(", "));
        show(&imp.speed, speed);
        show(&imp.ipv4, list(&detail.ipv4));
        show(&imp.ipv6, list(&detail.ipv6));
        show(&imp.hardware_address, detail.hardware_address.clone());
        show(&imp.driver, detail.driver.clone());
    }
}
