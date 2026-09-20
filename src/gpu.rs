//! Reads the graphics cards from /sys/class/drm. What there is to read depends on the
//! driver: amdgpu reports how busy a card is and its video memory, most drivers a
//! temperature, and some nothing at all.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Marketing names of AMD cards, by device and revision, as libdrm installs them.
const AMDGPU_IDS: &str = "/usr/share/libdrm/amdgpu.ids";
/// Names of PCI devices, as hwdata or pciutils install them.
const PCI_IDS: [&str; 2] = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"];
const AMD: u16 = 0x1002;

#[derive(Debug, Clone)]
pub struct Gpu {
    /// The PCI address of the card, which tells two of the same model apart.
    pub slot: Arc<str>,
    pub name: Option<Arc<str>>,
    /// Share of the last moment the card was busy, from 0 to 100.
    pub usage: Option<f64>,
    /// Video memory in use, and all there is, in bytes.
    pub memory: Option<(u64, u64)>,
    /// In degrees Celsius.
    pub temperature: Option<f64>,
}

/// A card found when the sampler started.
pub struct Card {
    device: PathBuf,
    slot: Arc<str>,
    name: Option<Arc<str>>,
    hwmon: Option<PathBuf>,
}

/// The display controllers behind /sys/class/drm, in the order the kernel numbered them.
pub fn cards() -> Vec<Card> {
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let mut numbered: Vec<(u32, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let number = entry
                .file_name()
                .to_str()?
                .strip_prefix("card")?
                .parse()
                .ok()?;
            Some((number, entry.path().join("device")))
        })
        .collect();
    numbered.sort_unstable_by_key(|(number, _)| *number);

    let mut cards: Vec<Card> = Vec::new();
    for (_, device) in numbered {
        // A framebuffer the firmware set up has a card too, but no PCI class.
        if !read(&device.join("class")).is_some_and(|class| class.starts_with("0x03")) {
            continue;
        }
        let Some(slot) = fs::canonicalize(&device)
            .ok()
            .and_then(|path| Some(Arc::from(path.file_name()?.to_str()?)))
        else {
            continue;
        };
        if cards.iter().any(|card| card.slot == slot) {
            continue;
        }
        let hwmon = fs::read_dir(device.join("hwmon"))
            .ok()
            .and_then(|mut entries| entries.next()?.ok())
            .map(|entry| entry.path());
        cards.push(Card {
            name: card_name(&device).map(Arc::from),
            device,
            slot,
            hwmon,
        });
    }
    cards
}

impl Card {
    pub fn read(&self) -> Gpu {
        let mut gpu = Gpu {
            slot: self.slot.clone(),
            name: self.name.clone(),
            usage: None,
            memory: None,
            temperature: None,
        };
        // Reading the driver's files wakes a card that has powered down while idle, as
        // the second card of a laptop does, and keeps it from ever powering down again.
        if read(&self.device.join("power/runtime_status")).as_deref() == Some("suspended") {
            gpu.usage = Some(0.0);
            return gpu;
        }
        gpu.usage = number(&self.device.join("gpu_busy_percent")).map(|busy| busy as f64);
        gpu.memory = number(&self.device.join("mem_info_vram_used"))
            .zip(number(&self.device.join("mem_info_vram_total")))
            .filter(|(_, total)| *total > 0);
        gpu.temperature = self
            .hwmon
            .as_ref()
            .and_then(|hwmon| number(&hwmon.join("temp1_input")))
            .map(|millidegrees| millidegrees as f64 / 1000.0);
        gpu
    }
}

fn card_name(device: &Path) -> Option<String> {
    let id = |file: &str| {
        let text = read(&device.join(file))?;
        u16::from_str_radix(text.trim_start_matches("0x"), 16).ok()
    };
    let (vendor, product) = (id("vendor")?, id("device")?);
    if vendor == AMD
        && let Some(revision) = id("revision")
        && let Some(name) = fs::read_to_string(AMDGPU_IDS)
            .ok()
            .and_then(|ids| amdgpu_name(&ids, product, revision))
    {
        return Some(name);
    }
    PCI_IDS
        .iter()
        .find_map(|path| fs::read_to_string(path).ok())
        .and_then(|ids| pci_name(&ids, vendor, product))
}

/// The name libdrm gives the AMD card with this device and revision.
fn amdgpu_name(ids: &str, device: u16, revision: u16) -> Option<String> {
    ids.lines().find_map(|line| {
        let mut fields = line.split(',').map(str::trim);
        let matches = u16::from_str_radix(fields.next()?, 16).ok()? == device
            && u16::from_str_radix(fields.next()?, 16).ok()? == revision;
        matches.then(|| fields.next().map(str::to_owned)).flatten()
    })
}

/// The name pci.ids gives the device: the product name in its brackets where it has one,
/// "Radeon RX 5700" of "Navi 10 [Radeon RX 5700]", as the rest is the chip's.
fn pci_name(ids: &str, vendor: u16, device: u16) -> Option<String> {
    let vendor = format!("{vendor:04x} ");
    let device = format!("\t{device:04x} ");
    let mut lines = ids.lines().skip_while(|line| !line.starts_with(&vendor));
    lines.next()?;
    let name = lines
        .take_while(|line| line.starts_with('\t') || line.starts_with('#'))
        .find_map(|line| line.strip_prefix(&device))?
        .trim();
    let product = name
        .rfind('[')
        .zip(name.strip_suffix(']'))
        .map(|(open, name)| &name[open + 1..]);
    Some(product.unwrap_or(name).to_owned())
}

fn read(path: &Path) -> Option<String> {
    let mut text = fs::read_to_string(path).ok()?;
    text.truncate(text.trim_end().len());
    Some(text)
}

fn number(path: &Path) -> Option<u64> {
    read(path)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PCI_IDS_EXCERPT: &str = "\
# List of PCI ID's
1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t67df  Ellesmere [Radeon RX 470/480/570/570X/580/580X/590]
\t\t1462 341b  Radeon RX 570 Armor 8G OC
# a comment within a vendor
\t731f  Navi 10 [Radeon RX 5600 OEM/5600 XT / 5700/5700 XT]
1003  ULSI Systems
\t0201  US201
8086  Intel Corporation
\t46a6  Alder Lake-P GT2 [Iris Xe Graphics]
\t0201  Arctic Sound
";

    #[test]
    fn cards_named_from_pci_ids() {
        assert_eq!(
            pci_name(PCI_IDS_EXCERPT, 0x1002, 0x731f).as_deref(),
            Some("Radeon RX 5600 OEM/5600 XT / 5700/5700 XT")
        );
        assert_eq!(
            pci_name(PCI_IDS_EXCERPT, 0x8086, 0x46a6).as_deref(),
            Some("Iris Xe Graphics")
        );
        // The same device number under another vendor is another device.
        assert_eq!(
            pci_name(PCI_IDS_EXCERPT, 0x8086, 0x0201).as_deref(),
            Some("Arctic Sound")
        );
        assert_eq!(pci_name(PCI_IDS_EXCERPT, 0x1002, 0x0201), None);
    }

    #[test]
    fn a_card_asleep_is_left_asleep() {
        let device = std::env::temp_dir().join(format!("locus-gpu-{}", std::process::id()));
        let hwmon = device.join("hwmon/hwmon3");
        fs::create_dir_all(device.join("power")).unwrap();
        fs::create_dir_all(&hwmon).unwrap();
        for (file, value) in [
            ("power/runtime_status", "active\n"),
            ("gpu_busy_percent", "37\n"),
            ("mem_info_vram_used", "20725760\n"),
            ("mem_info_vram_total", "8589934592\n"),
            ("hwmon/hwmon3/temp1_input", "44500\n"),
        ] {
            fs::write(device.join(file), value).unwrap();
        }
        let card = Card {
            device: device.clone(),
            slot: Arc::from("0000:2d:00.0"),
            name: None,
            hwmon: Some(hwmon),
        };

        let awake = card.read();
        assert_eq!(awake.usage, Some(37.0));
        assert_eq!(awake.memory, Some((20_725_760, 8_589_934_592)));
        assert_eq!(awake.temperature, Some(44.5));

        fs::write(device.join("power/runtime_status"), "suspended\n").unwrap();
        let asleep = card.read();
        fs::remove_dir_all(&device).unwrap();
        assert_eq!(asleep.usage, Some(0.0));
        assert_eq!((asleep.memory, asleep.temperature), (None, None));
    }

    #[test]
    fn amd_cards_named_by_revision() {
        let ids = "\
# device_id,\trevision_id,\tproduct_name
1.0.0
67DF,\tC7,\tAMD Radeon RX 480 Graphics
67DF,\tEF,\tAMD Radeon RX 570 Series
";
        assert_eq!(
            amdgpu_name(ids, 0x67df, 0xef).as_deref(),
            Some("AMD Radeon RX 570 Series")
        );
        assert_eq!(amdgpu_name(ids, 0x67df, 0xc0), None);
    }
}
