use std::path::PathBuf;

pub struct Config {
    pub max_line_width: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self { max_line_width: 80 }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        match path {
            Some(p) if p.exists() => Self::load_from_file(&p),
            _ => Self::default(),
        }
    }

    fn load_from_file(path: &std::path::Path) -> Self {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Warning: could not read config {}: {}", path.display(), e);
                return Self::default();
            }
        };

        let doc: kdl::KdlDocument = match content.parse() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Warning: invalid KDL in {}: {}", path.display(), e);
                return Self::default();
            }
        };

        let mut config = Self::default();

        if let Some(display) = doc.get("display")
            && let Some(children) = display.children()
            && let Some(node) = children.get("max-line-width")
            && let Some(val) = node.get(0).and_then(|v| v.as_integer())
        {
            config.max_line_width = val as usize;
        }

        config
    }
}

fn config_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("SKETCH_CONFIG") {
        return Some(PathBuf::from(p));
    }
    dirs::config_dir().map(|d| d.join("sketch").join("config.kdl"))
}
