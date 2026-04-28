//! Built-in registry of common Ruby tools so `rv tool add rubocop` works
//! without typing the gem name. Users can always specify `gem = "..."` in
//! `rv.toml` for things outside the registry.

#[derive(Debug, Clone, Copy)]
pub struct RegistryEntry {
    pub name: &'static str,
    pub gem: &'static str,
    pub bin: &'static str,
}

const ENTRIES: &[RegistryEntry] = &[
    RegistryEntry {
        name: "rubocop",
        gem: "rubocop",
        bin: "rubocop",
    },
    RegistryEntry {
        name: "standard",
        gem: "standard",
        bin: "standardrb",
    },
    RegistryEntry {
        name: "brakeman",
        gem: "brakeman",
        bin: "brakeman",
    },
    RegistryEntry {
        name: "steep",
        gem: "steep",
        bin: "steep",
    },
    RegistryEntry {
        name: "sorbet",
        gem: "sorbet",
        bin: "srb",
    },
    RegistryEntry {
        name: "ruby-lsp",
        gem: "ruby-lsp",
        bin: "ruby-lsp",
    },
    RegistryEntry {
        name: "solargraph",
        gem: "solargraph",
        bin: "solargraph",
    },
    RegistryEntry {
        name: "bundler",
        gem: "bundler",
        bin: "bundle",
    },
    RegistryEntry {
        name: "rake",
        gem: "rake",
        bin: "rake",
    },
    RegistryEntry {
        name: "rspec",
        gem: "rspec",
        bin: "rspec",
    },
    RegistryEntry {
        name: "rails",
        gem: "rails",
        bin: "rails",
    },
    RegistryEntry {
        name: "rerun",
        gem: "rerun",
        bin: "rerun",
    },
    RegistryEntry {
        name: "fasterer",
        gem: "fasterer",
        bin: "fasterer",
    },
    RegistryEntry {
        name: "reek",
        gem: "reek",
        bin: "reek",
    },
    RegistryEntry {
        name: "yard",
        gem: "yard",
        bin: "yard",
    },
];

pub fn lookup(name: &str) -> Option<RegistryEntry> {
    ENTRIES.iter().copied().find(|e| e.name == name)
}

pub fn all() -> &'static [RegistryEntry] {
    ENTRIES
}
