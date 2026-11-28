use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use wasm_bindgen_cli_support::Bindgen;

struct Config {
    input: PathBuf,
    out_dir: PathBuf,
    out_name: String,
    remove_name_section: bool,
}

fn parse_args<I>(args: I) -> Result<Config>
where
    I: IntoIterator<Item = String>,
{
    let mut iter = args.into_iter();
    let mut input = None;
    let mut out_dir = None;
    let mut out_name = None;
    let mut remove_name_section = false;

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--out-dir" => {
                let value = iter.next().context("missing value for --out-dir")?;
                out_dir = Some(PathBuf::from(value));
            }
            "--out-name" => {
                let value = iter.next().context("missing value for --out-name")?;
                out_name = Some(value);
            }
            "--remove-name-section" => remove_name_section = true,
            flag if flag.starts_with('-') => bail!("unknown flag: {flag}"),
            path => {
                if input.is_some() {
                    bail!("unexpected extra argument: {path}");
                }
                input = Some(PathBuf::from(path));
            }
        }
    }

    Ok(Config {
        input: input.context("missing input wasm path")?,
        out_dir: out_dir.context("missing --out-dir")?,
        out_name: out_name.context("missing --out-name")?,
        remove_name_section,
    })
}

fn run() -> Result<()> {
    let config = parse_args(env::args().skip(1))?;
    let mut bindgen = Bindgen::new();
    bindgen
        .input_path(&config.input)
        .web(true)?
        .omit_default_module_path(false)
        .out_name(&config.out_name)
        .remove_name_section(config.remove_name_section)
        .generate(&config.out_dir)
}

fn main() -> ExitCode {
    if let Err(err) = run() {
        eprintln!("meerai-web-bindgen: {err:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_expected_flags_and_input() {
        let cfg = parse_args([
            "--out-dir".into(),
            "target/web/.next".into(),
            "--out-name".into(),
            "app".into(),
            "--remove-name-section".into(),
            "app.wasm".into(),
        ])
        .unwrap();

        assert_eq!(cfg.out_dir, PathBuf::from("target/web/.next"));
        assert_eq!(cfg.out_name, "app");
        assert!(cfg.remove_name_section);
        assert_eq!(cfg.input, PathBuf::from("app.wasm"));
    }

    #[test]
    fn rejects_unknown_flag() {
        let err = parse_args(["--bogus".into()]).err().unwrap();
        assert!(err.to_string().contains("unknown flag"));
    }

    #[test]
    fn rejects_missing_required_arguments() {
        assert!(parse_args([]).is_err());
        assert!(parse_args(["app.wasm".into()]).is_err());
    }
}
