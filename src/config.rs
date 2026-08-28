pub const APP_ID: &str = "io.github.sachesi.locus";
pub const RESOURCE_PATH: &str = "/io/github/sachesi/locus";
pub const GETTEXT_PACKAGE: &str = "locus";

/// Where `just install` puts the message catalogues, fixed at build time so a build for
/// a different prefix finds its own translations.
pub const LOCALEDIR: &str = match option_env!("LOCUS_LOCALEDIR") {
    Some(dir) => dir,
    None => "/usr/share/locale",
};
