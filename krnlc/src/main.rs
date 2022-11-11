
use std::{process::Command, collections::{HashMap, BTreeMap}, path::{Path, PathBuf}, fs, borrow::Cow, ffi::OsStr};
use clap_cargo::{Features, Manifest, Workspace};
use anyhow::{Result, format_err, bail};
use syn::{visit::Visit, ItemConst, Lit, Expr, ItemMod, File};
use cargo_metadata::{Metadata, MetadataCommand, Package};
use spirv_builder::{SpirvBuilder, MetadataPrintout, SpirvMetadata, ModuleResult};
use quote::quote;


#[derive(clap::Parser, Debug)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd
}

#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// collects and compiles all modules
    ///
    /// - writes to [<manifest-dir>]/.krnl/[<package>]/cache
    Build {
        #[command(flatten)]
        workspace: Workspace,
        #[command(flatten)]
        features: Features,
        /// Directory for all generated artifacts
        #[arg(long, name="target-dir", value_name = "DIRECTORY")]
        target_dir: Option<String>,
        #[command(flatten)]
        manifest: Manifest,
    },
    /// removes files created by krnlc
    ///
    ///    - build artifacts via cargo clean for each package
    ///
    ///    - [<manifest-dir>]/.krnl/[<package>]
    ///
    ///    - [<manifest-dir>]/.krnl if all packages are removed
    Clean {
        #[command(flatten)]
        workspace: Workspace,
        /// Directory for all generated artifacts
        #[arg(long, name="target-dir", value_name = "DIRECTORY")]
        target_dir: Option<String>,
        #[command(flatten)]
        manifest: Manifest,
    }
}

fn main() -> Result<()> {
    use clap::Parser;
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Build {
            workspace,
            features,
            target_dir,
            manifest,
        } => build(workspace, features, target_dir, manifest),
        Cmd::Clean {
            ..
        } => {
            todo!()
        }
    }
    /*
    let package = get_root_package(&metadata)?;
    let deps = package.dependencies.iter().map(|x| x.name.as_str());
    run_check(&args.features, deps)?;
    let crate_name = package.name.replace('-', "_");
    let modules = run_expand(crate_name, &args.features)?;
    Ok(())*/
}

fn build(workspace: Workspace, features: Features, target_dir: Option<String>, manifest: Manifest) -> Result<()> {
    let metadata = manifest.metadata().exec()?;
    let (selected, _) = workspace.partition_packages(&metadata);
    let krnl_dir = PathBuf::from(".krnl");
    init_krnl_dir(&krnl_dir)?;
    let packages_dir = krnl_dir.join("packages");
    if !packages_dir.exists() {
        fs::create_dir(&packages_dir)?;
    }
    for package in selected {
        let deps = package.dependencies.iter().map(|x| x.name.as_str());
        let manifest_path = package.manifest_path.as_str();
        cargo_check(&features, target_dir.as_deref(), manifest_path, deps)?;
        let module_datas = cargo_expand(&package.name, &features, target_dir.as_deref(), manifest_path)?;
        let target_dir = if let Some(target_dir) = target_dir.as_ref() {
            target_dir.into()
        } else {
            package.manifest_path.parent().unwrap().as_std_path().join("target")
        };
        let package_dir = packages_dir.join(&package.name);
        if !package_dir.exists() {
            fs::create_dir(&package_dir)?;
        }
        fs::write(
            package_dir.join(".gitignore"),
            "# Generated by krnlc\nmodules".as_bytes()
        )?;
        let modules_dir = package_dir.join("modules");
        if !modules_dir.exists() {
            fs::create_dir(&modules_dir)?;
        }
        let mut kernels = Vec::with_capacity(module_datas.len());
        for module_data in module_datas.iter() {
            kernels.push(compile(&modules_dir, module_data, target_dir.to_string_lossy().as_ref())?);
        }
        cache(&package_dir, &package.name, &module_datas, &kernels)?;
    }
    Ok(())
}

fn add_features_to_command(command: &mut Command, features: &Features) -> Result<()> {
    if features.all_features {
        command.arg("--all-features");
    }
    if features.no_default_features {
        command.arg("--no-default-features");
    }
    match features.features.as_slice() {
        [] => (),
        [feature] => {
            command.args(["--features", feature]);
        }
        [feature, ..] => {
            command.arg("--features");
            let mut features_string = format!("\"{feature}");
            use std::fmt::Write;
            for feature in features.features.iter().skip(1) {
                write!(&mut features_string, " {feature}")?;
            }
            features_string.push('\"');
            command.arg(&features_string);
        }
    }
    Ok(())
}
/*
fn get_root_package(metadata: &Metadata) -> Result<&Package> {
    let root = metadata.resolve.as_ref().map(|x| x.root.as_ref()).flatten();
    if let Some(package) = metadata.packages.iter().find(|x| Some(&x.id) == root) {
        Ok(package)
    } else {
        Err(format_err!("Unable to parse root package!"))
    }
}*/

fn cargo_check<'a>(features: &Features, target_dir: Option<&str>, manifest_path: &str, deps: impl Iterator<Item=&'a str>) -> Result<()> {
    let mut command = Command::new("cargo");
    command.args(["+nightly", "check", "--manifest-path", manifest_path]);
    add_features_to_command(&mut command, features)?;
    if let Some(target_dir) = target_dir {
        command.args(&["--target-dir", target_dir]);
    }
    for dep in deps {
        command.args(&["-p", dep]);
    }
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format_err!("cargo check failed!"))
    }
}

fn cargo_expand(crate_name: &str, features: &Features, target_dir: Option<&str>, manifest_path: &str) -> Result<Vec<ModuleData>> {
    let mut command = Command::new("cargo");
    command.args(["+nightly", "rustc", "--manifest-path", manifest_path]);
    add_features_to_command(&mut command, features)?;
    if let Some(target_dir) = target_dir {
        command.args(&["--target-dir", target_dir]);
    }
    command.args(&[
        "--profile=check",
        "--",
        "-Zunpretty=expanded",
    ]);
    let output = command.output()?;
    let expanded = std::str::from_utf8(&output.stdout)?;
    let file = syn::parse_str(expanded)?;
    let mut modules = Vec::new();
    let mut visitor = Visitor {
        path: crate_name.replace('-',"_"),
        modules: &mut modules,
    };
    visitor.visit_file(&file);
    Ok(modules)
    /*
    let output_dir = std::env::current_dir()?;
    let target_dir = output_dir.join("target");
    let krnl_dir = init_krnl_dir(&output_dir)?;
    for module_data in modules.iter() {
        compile(&krnl_dir, &module_data, &target_dir)?;
    }
    todo!()*/
}

#[derive(Debug)]
struct ModuleData {
    path: String,
    data: HashMap<String, String>,
}

struct Visitor<'a> {
    path: String,
    modules: &'a mut Vec<ModuleData>,
}


impl<'a, 'ast> Visit<'ast> for Visitor<'a> {
    fn visit_item_mod(&mut self, i: &'ast ItemMod) {
        let mut visitor = Visitor {
            path: format!("{}::{}", self.path, i.ident),
            modules: &mut self.modules,
        };
        syn::visit::visit_item_mod(&mut visitor, i);
    }
    fn visit_item_const(&mut self, i: &'ast ItemConst) {
        if let Some(path) = self.path.strip_suffix("::module") {
            if i.ident == "krnlc__krnl_module_data" {
                if let Expr::Array(expr_array) = &*i.expr {
                    let mut bytes = Vec::<u8>::with_capacity(expr_array.elems.len());
                    for elem in expr_array.elems.iter() {
                        if let Expr::Lit(expr_lit) = elem {
                            if let Lit::Int(lit_int) = &expr_lit.lit {
                                if let Ok(val) = lit_int.base10_parse() {
                                    bytes.push(val);
                                } else {
                                    return;
                                }
                            }
                        } else {
                            return;
                        }
                    }
                    if let Ok(data) = bincode::deserialize::<HashMap<String, String>>(&bytes) {
                        if data.contains_key("krnl_module_tokens") {
                            let data = ModuleData {
                                path: path.to_string(),
                                data,
                            };
                            self.modules.push(data);
                        }
                    }

                }
            }
        }
    }
}

fn init_krnl_dir(krnl_dir: &Path) -> Result<()> {
    if !krnl_dir.exists() {
        fs::create_dir(&krnl_dir)?;
    }
    let cashdir_tag_path = krnl_dir.join("CASHDIR.TAG");
    let cashdir_tag = concat!(
        "Signature: 8a477f597d28d172789f06886806bc55",
        "\n# This file is a cache directory tag created by krnlc.",
        "\n# For information about cache directory tags see https://bford.info/cachedir/"
    );
    if !cashdir_tag_path.exists() {
        fs::write(&cashdir_tag_path, cashdir_tag.as_bytes())?;
    } else {
        let tag = fs::read_to_string(&cashdir_tag_path)?;
        if tag != cashdir_tag {
            let path = cashdir_tag_path.to_string_lossy();
            bail!("A CASHDIR.TAG already exists at {path:?} for another app!");
        }
    }
    fs::write(
        krnl_dir.join(".gitignore"),
        "# Generated by krnlc\nlib".as_bytes()
    )?;
    let lib_dir = krnl_dir.join("lib");
    if !lib_dir.exists() {
        fs::create_dir(&lib_dir)?;
    }
    let librustc_codegen_spirv = include_bytes!(concat!(env!("OUT_DIR"), "/../../../librustc_codegen_spirv.so"));
    fs::write(lib_dir.join("librustc_codegen_spirv.so"), librustc_codegen_spirv.as_ref())?;
    // https://github.com/EmbarkStudios/rust-gpu/blob/main/crates/spirv-builder/src/lib.rs
    fn dylib_path_envvar() -> &'static str {
        if cfg!(windows) {
            "PATH"
        } else if cfg!(target_os = "macos") {
            "DYLD_FALLBACK_LIBRARY_PATH"
        } else {
            "LD_LIBRARY_PATH"
        }
    }
    let lib_dir = lib_dir.canonicalize()?;
    let path_var = dylib_path_envvar();
    let path = if let Ok(path) = std::env::var(path_var) {
        std::env::join_paths(std::iter::once(lib_dir).chain(std::env::split_paths(&path)))?
    } else {
        lib_dir.into_os_string()
    };
    std::env::set_var(path_var, path);
    Ok(())
}

fn compile(modules_dir: &Path, module_data: &ModuleData, target_dir: &str) -> Result<BTreeMap<String, PathBuf>> {
    let crate_name = module_data.path.replace("::", "_");
    let crate_dir = modules_dir.join(&crate_name);
    if !crate_dir.exists() {
        fs::create_dir(&crate_dir)?;
    }
    let dependencies = module_data.data.get("dependencies").unwrap();
    let mut manifest = format!(
r#"[package]
name = {crate_name:?}
version = "0.1.0"
edition = "2021"
publish = false

[lib]
crate-type = ["dylib"]

[dependencies]
{dependencies}

[patch.crates-io]
libm = {{ git = "https://github.com/rust-lang/libm", tag = "0.2.5" }}
"#
    );
    fs::write(
        crate_dir.join("Cargo.toml"),
        manifest.as_bytes()
    )?;
    let cargo_dir = crate_dir.join(".cargo");
    if !cargo_dir.exists() {
        fs::create_dir(&cargo_dir)?;
    }
    let config = format!("[build]\ntarget-dir = {target_dir:?}");
    fs::write(cargo_dir.join("config.toml"), config.as_bytes())?;
    let toolchain = r#"[toolchain]
channel = "nightly-2022-08-29"
components = ["rust-src", "rustc-dev", "llvm-tools-preview"]"#;
    fs::write(
        crate_dir.join("rust-toolchain.toml"),
        toolchain.as_bytes()
    )?;
    let src_dir = crate_dir.join("src");
    if !src_dir.exists() {
        fs::create_dir(&src_dir)?;
    }
    let tokens = module_data.data.get("krnl_module_tokens").unwrap();
    let file = syn::parse_str(tokens)?;
    let src = prettyplease::unparse(&file);
    let src = format!(r#"#![cfg_attr(target_arch = "spirv",
no_std,
feature(register_attr),
register_attr(spirv)
)]
{src}
extern crate spirv_std; "#);
    fs::write(src_dir.join("lib.rs"), src.as_bytes())?;
    let status = Command::new("cargo")
        .args(&[
            "update",
            "--manifest-path",
            crate_dir.join("Cargo.toml").to_string_lossy().as_ref(),
        ])
        .status()?;
    if !status.success() {
        bail!("cargo update failed!");
    }
    let module = SpirvBuilder::new(&crate_dir, "spirv-unknown-vulkan1.2")
        .multimodule(true)
        .spirv_metadata(SpirvMetadata::NameVariables)
        .print_metadata(MetadataPrintout::None)
        .deny_warnings(true)
        .preserve_bindings(true)
        .build()?
        .module;
    if let ModuleResult::MultiModule(map) = module {
        Ok(map)
    } else {
        Err(format_err!("Expected multimodule!"))
    }
}

fn cache(package_dir: &Path, package_name: &str, module_datas: &[ModuleData], kernels: &[BTreeMap<String, PathBuf>]) -> Result<()> {
    let mut module_arms = Vec::with_capacity(module_datas.len());
    //let mut kernel_arms = Vec::with_capacity(kernels.iter().map(|x| x.len()).sum());
    for (module_data, kernels) in module_datas.iter().zip(kernels) {
        let module_path = &module_data.path;
        let dependencies = module_data.data.get("dependencies").unwrap();
        let module_tokens = module_data.data.get("krnl_module_tokens").unwrap();
        let module_src = format!("(dependencies({dependencies:?})) => ({module_tokens})");
        let module_path_indices = (0 .. module_path.len()).into_iter().collect::<Vec<_>>();
        module_arms.push(quote! {
            {
                let module_path = #module_path.as_bytes();
                if path.len() == module_path.len() + "::module".len() {
                    let success = #(path[#module_path_indices] == module_path[#module_path_indices])&&*;
                    if success {
                        return Some(#module_src);
                    }
                }
            }
        });
        /*for (kernel, spirv_path) in kernels.iter() {
            kernel_arms.push(quote! {
                #kernel_path => #kernel_data,
            });
        }*/
    }
    let cache = quote! {
        /* generated by krnlc */

        const fn __module(path: &'static str) -> Option<&'static str> {
            let path = path.as_bytes();
            #(#module_arms)*
            None
        }
        /*const fn __kernel(path: &'static str) -> Option<&'static [u8]> {
            match path {
                #kernel_arms
                _ => None,
            }
        }*/
    }.to_string();
    fs::write(package_dir.join("cache"), cache.as_bytes())?;
    Ok(())
}
