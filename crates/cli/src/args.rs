//! Command-line parsing for `aigle export`.

use std::path::PathBuf;

use aigle_engine_api::{
    Anchor, DisplayProfileId, EncodedFormat, MetadataPolicy, OutputSharpening, Watermark,
    WatermarkContent,
};

pub const HELP: &str = "\
aigle: raw photo editor, command line

Usage:
  aigle export [OPTIONS] -o DIR PHOTO|FOLDER...
  aigle --help | --version

Export renders each photo with the same settings and writes it to DIR. Folders are
expanded to the photos directly inside them.

Edits:
      --params FILE           EditParams JSON applied to every photo (default: as shot)
      --sidecar               use each photo's own edits from its XMP sidecar (read only);
                              photos without a sidecar use --params or the defaults

Output:
  -o, --output DIR            destination folder, created if missing (required)
  -f, --format NAME           jpeg (default), png, tiff, tiff16, webp, avif, heic, jxl, dng
  -q, --quality N             1-100 for jpeg, webp, avif, heic and jxl (default 90)
      --long-edge PX          resize so the long edge is at most PX (default: full size)
      --color-space NAME      srgb (default), p3, adobe-rgb, prophoto, rec2020
      --sharpen LEVEL         off (default), low, standard, high
      --metadata WHICH        all (default), copyright, none
      --artist TEXT           replaces the camera's artist
      --copyright TEXT        replaces the camera's copyright
      --name TEMPLATE         file name without extension; {name} is the photo's name,
                              {seq} its number in the batch (default {name})
      --overwrite             replace existing files instead of numbering new ones -2, -3...

Watermark (not on DNG):
      --watermark-text TEXT   draw TEXT
      --watermark-image FILE  draw an image (PNG, SVG...)
      --watermark-size PCT    height in percent of the short edge (default 4)
      --watermark-opacity PCT 1-100 (default 60)
      --watermark-margin PCT  distance from the edges in percent of the short edge (default 3)
      --watermark-position P  top-left, top, top-right, left, center, right, bottom-left,
                              bottom, bottom-right (default)
      --watermark-color C     white (default), black, gray or #rrggbb

Running:
  -j, --jobs N                photos in flight at most (default: all the memory budget
                              allows, up to the number of CPU threads)
      --cpu                   render on the CPU even if a GPU is usable
      --memory MIB            engine memory budget
      --json                  print the summary as JSON on stdout
  -h, --help                  show this help

DNG export keeps the raw sensor data; the edits go to the DNG's XMP sidecar.
Exit status: 0 all exported, 1 some failed, 2 bad arguments, 130 interrupted.
";

#[derive(Clone, Debug, PartialEq)]
pub struct Export {
    pub inputs: Vec<PathBuf>,
    pub output: PathBuf,
    pub format: EncodedFormat,
    pub long_edge: Option<u32>,
    pub color_space: DisplayProfileId,
    pub sharpening: OutputSharpening,
    pub metadata: MetadataPolicy,
    pub artist: Option<String>,
    pub copyright: Option<String>,
    pub template: String,
    pub overwrite: bool,
    pub params: Option<PathBuf>,
    pub sidecar: bool,
    pub watermark: Option<Watermark>,
    pub jobs: Option<usize>,
    pub cpu: bool,
    pub memory_mib: Option<u64>,
    pub json: bool,
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Version,
    Export(Box<Export>),
}

fn format(name: &str, quality: u8) -> Result<EncodedFormat, String> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "jpeg" | "jpg" => EncodedFormat::Jpeg { quality },
        "png" => EncodedFormat::Png,
        "tiff" | "tif" => EncodedFormat::Tiff { sixteen_bit: false },
        "tiff16" | "tif16" => EncodedFormat::Tiff { sixteen_bit: true },
        "webp" => EncodedFormat::WebP { quality },
        "avif" => EncodedFormat::Avif { quality },
        "heic" | "heif" => EncodedFormat::Heic { quality },
        "jxl" | "jpeg-xl" => EncodedFormat::Jxl { quality },
        "dng" => EncodedFormat::Dng,
        _ => return Err(format!("unknown format `{name}`")),
    })
}

fn color_space(name: &str) -> Result<DisplayProfileId, String> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "srgb" => DisplayProfileId::Srgb,
        "p3" | "display-p3" => DisplayProfileId::DisplayP3,
        "adobe-rgb" | "adobergb" => DisplayProfileId::AdobeRgb,
        "prophoto" => DisplayProfileId::ProPhoto,
        "rec2020" => DisplayProfileId::Rec2020,
        _ => return Err(format!("unknown color space `{name}`")),
    })
}

fn anchor(name: &str) -> Result<Anchor, String> {
    const NAMES: [&str; 9] = [
        "top-left",
        "top",
        "top-right",
        "left",
        "center",
        "right",
        "bottom-left",
        "bottom",
        "bottom-right",
    ];
    NAMES
        .iter()
        .position(|n| n.eq_ignore_ascii_case(name))
        .map(|i| Anchor::ALL[i])
        .ok_or_else(|| format!("unknown watermark position `{name}`"))
}

fn color(name: &str) -> Result<[u8; 3], String> {
    match name.to_ascii_lowercase().as_str() {
        "white" => Ok([255; 3]),
        "black" => Ok([0; 3]),
        "gray" | "grey" => Ok([128; 3]),
        s if s.len() == 7 && s.starts_with('#') => {
            let v = u32::from_str_radix(&s[1..], 16).map_err(|_| format!("bad color `{name}`"))?;
            Ok([(v >> 16) as u8, (v >> 8) as u8, v as u8])
        }
        _ => Err(format!("unknown color `{name}`")),
    }
}

fn number<T: std::str::FromStr>(opt: &str, v: &str) -> Result<T, String> {
    v.parse()
        .map_err(|_| format!("{opt}: `{v}` is not a valid number"))
}

fn percent(opt: &str, v: &str, min: f32, max: f32) -> Result<f32, String> {
    let p: f32 = number(opt, v)?;
    if !(min..=max).contains(&p) {
        return Err(format!("{opt}: {p} is outside {min}-{max}"));
    }
    Ok(p / 100.0)
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        None | Some("-h" | "--help" | "help") => return Ok(Command::Help),
        Some("-V" | "--version") => return Ok(Command::Version),
        Some("export") => {}
        Some(other) => return Err(format!("unknown command `{other}`")),
    }
    let mut inputs = Vec::new();
    let mut output = None;
    let mut fmt = "jpeg".to_string();
    let mut quality = 90u8;
    let mut e = Export {
        inputs: Vec::new(),
        output: PathBuf::new(),
        format: EncodedFormat::default(),
        long_edge: None,
        color_space: DisplayProfileId::Srgb,
        sharpening: OutputSharpening::Off,
        metadata: MetadataPolicy::All,
        artist: None,
        copyright: None,
        template: "{name}".into(),
        overwrite: false,
        params: None,
        sidecar: false,
        watermark: None,
        jobs: None,
        cpu: false,
        memory_mib: None,
        json: false,
    };
    let mut wm = Watermark::default();
    let mut wm_content = None;
    let mut options_done = false;
    while let Some(arg) = args.next() {
        if options_done || !arg.starts_with('-') || arg == "-" {
            inputs.push(PathBuf::from(arg));
            continue;
        }
        if arg == "--" {
            options_done = true;
            continue;
        }
        let (opt, inline) = match arg.split_once('=') {
            Some((o, v)) if o.starts_with("--") => (o.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = || -> Result<String, String> {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| format!("{opt} needs a value"))
        };
        match opt.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-o" | "--output" => output = Some(PathBuf::from(value()?)),
            "-f" | "--format" => fmt = value()?,
            "-q" | "--quality" => {
                quality = number(&opt, &value()?)?;
                if !(1..=100).contains(&quality) {
                    return Err(format!("{opt}: quality must be 1-100"));
                }
            }
            "--long-edge" => {
                let v: u32 = number(&opt, &value()?)?;
                if v == 0 {
                    return Err(format!("{opt} must be positive"));
                }
                e.long_edge = Some(v);
            }
            "--color-space" => e.color_space = color_space(&value()?)?,
            "--sharpen" => {
                e.sharpening = match value()?.to_ascii_lowercase().as_str() {
                    "off" => OutputSharpening::Off,
                    "low" => OutputSharpening::Low,
                    "standard" => OutputSharpening::Standard,
                    "high" => OutputSharpening::High,
                    v => return Err(format!("unknown sharpening `{v}`")),
                }
            }
            "--metadata" => {
                e.metadata = match value()?.to_ascii_lowercase().as_str() {
                    "all" => MetadataPolicy::All,
                    "copyright" => MetadataPolicy::CopyrightOnly,
                    "none" => MetadataPolicy::Strip,
                    v => return Err(format!("unknown metadata choice `{v}`")),
                }
            }
            "--artist" => e.artist = Some(value()?),
            "--copyright" => e.copyright = Some(value()?),
            "--name" => e.template = value()?,
            "--overwrite" => e.overwrite = true,
            "--params" => e.params = Some(PathBuf::from(value()?)),
            "--sidecar" => e.sidecar = true,
            "--watermark-text" => wm_content = Some(WatermarkContent::Text(value()?)),
            "--watermark-image" => {
                wm_content = Some(WatermarkContent::Image(PathBuf::from(value()?)))
            }
            "--watermark-size" => wm.size = percent(&opt, &value()?, 0.1, 100.0)?,
            "--watermark-opacity" => wm.opacity = percent(&opt, &value()?, 1.0, 100.0)?,
            "--watermark-margin" => wm.margin = percent(&opt, &value()?, 0.0, 45.0)?,
            "--watermark-position" => wm.anchor = anchor(&value()?)?,
            "--watermark-color" => wm.color = color(&value()?)?,
            "-j" | "--jobs" => {
                let n: usize = number(&opt, &value()?)?;
                if n == 0 {
                    return Err(format!("{opt} must be at least 1"));
                }
                e.jobs = Some(n);
            }
            "--cpu" => e.cpu = true,
            "--memory" => e.memory_mib = Some(number(&opt, &value()?)?),
            "--json" => e.json = true,
            _ => return Err(format!("unknown option `{opt}`")),
        }
    }
    e.format = format(&fmt, quality)?;
    e.output = output.ok_or("missing --output DIR")?;
    if inputs.is_empty() {
        return Err("no photos given".into());
    }
    e.inputs = inputs;
    if let Some(content) = wm_content {
        if e.format == EncodedFormat::Dng {
            return Err("DNG export cannot have a watermark".into());
        }
        if let WatermarkContent::Text(t) = &content
            && t.trim().is_empty()
        {
            return Err("--watermark-text is empty".into());
        }
        wm.content = content;
        e.watermark = Some(wm);
    }
    Ok(Command::Export(Box::new(e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export(args: &[&str]) -> Result<Export, String> {
        let mut v = vec!["export".to_string()];
        v.extend(args.iter().map(|s| s.to_string()));
        match parse(v)? {
            Command::Export(e) => Ok(*e),
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn defaults_and_values() {
        let e = export(&["a.nef", "-o", "out", "b.cr3"]).unwrap();
        assert_eq!(e.inputs, [PathBuf::from("a.nef"), PathBuf::from("b.cr3")]);
        assert_eq!(e.format, EncodedFormat::Jpeg { quality: 90 });
        assert_eq!((e.template.as_str(), e.watermark), ("{name}", None));
        let e = export(&[
            "--format=avif",
            "-q",
            "70",
            "--long-edge",
            "2048",
            "--color-space",
            "p3",
            "--sharpen=standard",
            "--metadata",
            "copyright",
            "-j",
            "3",
            "--output=o",
            "--",
            "-odd-name.raf",
        ])
        .unwrap();
        assert_eq!(e.format, EncodedFormat::Avif { quality: 70 });
        assert_eq!(e.long_edge, Some(2048));
        assert_eq!(e.color_space, DisplayProfileId::DisplayP3);
        assert_eq!(e.sharpening, OutputSharpening::Standard);
        assert_eq!(e.metadata, MetadataPolicy::CopyrightOnly);
        assert_eq!(e.jobs, Some(3));
        assert_eq!(e.inputs, [PathBuf::from("-odd-name.raf")]);
    }

    #[test]
    fn watermark_options() {
        let e = export(&[
            "x.nef",
            "-o",
            "o",
            "--watermark-text",
            "© Me",
            "--watermark-size",
            "8",
            "--watermark-position",
            "top-left",
            "--watermark-color",
            "#ff8000",
        ])
        .unwrap();
        let w = e.watermark.unwrap();
        assert_eq!(w.content, WatermarkContent::Text("© Me".into()));
        assert!((w.size - 0.08).abs() < 1e-6);
        assert_eq!((w.anchor, w.color), (Anchor::TopLeft, [255, 128, 0]));
    }

    #[test]
    fn mistakes_are_reported() {
        assert!(export(&["a.nef"]).unwrap_err().contains("--output"));
        assert!(export(&["-o", "o"]).unwrap_err().contains("no photos"));
        assert!(export(&["a", "-o", "o", "-f", "gif"]).is_err());
        assert!(export(&["a", "-o", "o", "-q", "0"]).is_err());
        assert!(export(&["a", "-o", "o", "--bogus"]).is_err());
        assert!(export(&["a", "-o"]).unwrap_err().contains("needs a value"));
        assert!(export(&["a", "-o", "o", "-f", "dng", "--watermark-text", "x"]).is_err());
        assert_eq!(parse(Vec::<String>::new()), Ok(Command::Help));
        assert!(parse(["frobnicate".to_string()]).is_err());
    }
}
