use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use futures::future::try_join_all;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rv_core::install::{self as ruby_install, InstallReport};
use rv_core::lock::{Lock, LockedTool};
use rv_core::manifest::VersionSource;
use rv_core::paths::Paths;
use rv_core::platform::Platform;
use rv_core::project::{self, ToolSpec};
use rv_core::{resolve, tool};
use sha2::Digest;

#[derive(Debug, Parser)]
#[command(
    name = "rv",
    version,
    about = "Ruby version & gem manager. uv-grade speed.",
    propagate_version = true
)]
struct Cli {
    #[arg(short = 'q', long = "quiet", global = true)]
    quiet: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Install a Ruby (e.g. `rv install 3.3.5`). Shells out to ruby-build.
    Install { version: String },
    /// List installed Rubies, or every installable definition with --remote.
    List {
        #[arg(long)]
        remote: bool,
    },
    /// Show the resolved Ruby version + the source of that decision.
    Current,
    /// Print the path of `ruby` (or another binary) for this project.
    Which {
        #[arg(default_value = "ruby")]
        tool: String,
    },
    /// Set `~/.config/rv/global` so it wins when no project file pins one.
    UseGlobal { version: String },
    /// Run a command using the resolved Ruby (and pinned tools' GEM_HOME).
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        argv: Vec<String>,
    },
    /// Pin a tool in rv.toml + install. `name` or `name@version`.
    Add {
        #[command(subcommand)]
        target: AddCmd,
    },
    /// Reconcile installs with rv.toml / rv.lock.
    Sync {
        #[arg(long)]
        frozen: bool,
    },
    /// Initialize rv.toml in the current directory.
    Init {
        #[arg(long, value_delimiter = ',')]
        with: Option<Vec<String>>,
        #[arg(long)]
        ruby: Option<String>,
        #[arg(long)]
        force: bool,
    },
    /// Manage gems pinned in this project.
    Tool {
        #[command(subcommand)]
        op: ToolCmd,
    },
    /// Run a tool ephemerally without pinning. argv[0]=`rvx` dispatches here.
    X {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1..)]
        argv: Vec<String>,
    },
    /// Drift report: pinned tools / Ruby behind their latest available.
    Outdated,
    /// Re-resolve rv.toml against rubygems.org and rewrite rv.lock without
    /// installing.
    Lock,
    /// Re-resolve pinned tools (and optionally Ruby) to their latest matching
    /// versions.
    Upgrade {
        names: Vec<String>,
        #[arg(long)]
        ruby: bool,
    },
    /// Print the resolved environment as a tree.
    Tree,
    /// Inspect or prune the rv data directories.
    Cache {
        #[command(subcommand)]
        op: CacheCmd,
    },
    /// One-line path query for shell substitution: `rv dir tools` etc.
    Dir {
        #[arg(value_enum)]
        kind: DirKind,
    },
    /// Drop a Ruby toolchain.
    Uninstall { version: String },
    /// Shell-evaluable exports for the resolved environment.
    Env {
        #[arg(long, value_enum, default_value_t = EnvShell::Sh)]
        shell: EnvShell,
    },
    /// Update rv to the latest release.
    SelfUpdate {
        #[arg(long)]
        check: bool,
    },
    /// Generate shell completions.
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Health check.
    Doctor,
}

#[derive(Debug, Subcommand)]
enum AddCmd {
    /// `rv add tool rubocop` or `rv add tool rubocop@1.65.0`.
    Tool { spec: String },
}

#[derive(Debug, Subcommand)]
enum ToolCmd {
    #[command(visible_alias = "ls")]
    List,
    Registry,
    Add {
        spec: String,
    },
    Remove {
        name: String,
    },
}

#[derive(Debug, Subcommand)]
enum CacheCmd {
    Info,
    Prune {
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum EnvShell {
    Sh,
    Fish,
    Powershell,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum DirKind {
    Data,
    Cache,
    Config,
    Versions,
    Tools,
}

static QUIET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn quiet() -> bool {
    QUIET.load(std::sync::atomic::Ordering::Relaxed)
}
macro_rules! say {
    ($($arg:tt)*) => {{ if !quiet() { println!($($arg)*); } }};
}

fn main() -> ExitCode {
    let argv0 = std::env::args_os()
        .next()
        .and_then(|p| {
            Path::new(&p)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    let cli = if argv0 == "rvx" {
        let injected = std::iter::once("rv".to_string())
            .chain(std::iter::once("x".to_string()))
            .chain(std::env::args().skip(1));
        Cli::parse_from(injected)
    } else {
        Cli::parse()
    };
    QUIET.store(cli.quiet, std::sync::atomic::Ordering::Relaxed);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    match rt.block_on(run(cli)) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(1)
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode> {
    let paths = Paths::discover()?;
    paths.ensure_dirs()?;
    let _platform = Platform::detect()?;

    match cli.cmd {
        Cmd::Install { version } => cmd_install(&paths, &version),
        Cmd::List { remote } => cmd_list(&paths, remote),
        Cmd::Current => cmd_current(&paths),
        Cmd::Which { tool } => cmd_which(&paths, &tool),
        Cmd::UseGlobal { version } => cmd_use_global(&paths, &version),
        Cmd::Run { argv } => cmd_run(&paths, argv),
        Cmd::Add { target } => match target {
            AddCmd::Tool { spec } => cmd_add_tool(&paths, &spec).await,
        },
        Cmd::Sync { frozen } => cmd_sync(&paths, frozen).await,
        Cmd::Init { with, ruby, force } => cmd_init(with, ruby, force),
        Cmd::Tool { op } => match op {
            ToolCmd::List => cmd_tool_list(&paths),
            ToolCmd::Registry => cmd_tool_registry(),
            ToolCmd::Add { spec } => cmd_add_tool(&paths, &spec).await,
            ToolCmd::Remove { name } => cmd_tool_remove(&paths, &name),
        },
        Cmd::X { argv } => cmd_x(&paths, argv).await,
        Cmd::Outdated => cmd_outdated(&paths).await,
        Cmd::Lock => cmd_lock(&paths).await,
        Cmd::Upgrade { names, ruby } => cmd_upgrade(&paths, names, ruby).await,
        Cmd::Tree => cmd_tree(&paths),
        Cmd::Cache { op } => match op {
            CacheCmd::Info => cmd_cache_info(&paths),
            CacheCmd::Prune { dry_run } => cmd_cache_prune(&paths, dry_run),
        },
        Cmd::Dir { kind } => cmd_dir(&paths, kind),
        Cmd::Uninstall { version } => cmd_uninstall(&paths, &version),
        Cmd::Env { shell } => cmd_env(&paths, shell),
        Cmd::SelfUpdate { check } => cmd_self_update(check).await,
        Cmd::Completions { shell } => cmd_completions(shell),
        Cmd::Doctor => cmd_doctor(&paths),
    }
}

fn cmd_install(paths: &Paths, version: &str) -> Result<ExitCode> {
    let pb = spinner(&format!("installing ruby {version} (ruby-build)"));
    let report: InstallReport = ruby_install::install(paths, version)?;
    pb.finish_and_clear();
    if report.already_present {
        say!(
            "{} ruby {} {}",
            success_mark(),
            report.version,
            dim("(already present)")
        );
    } else {
        say!("{} installed ruby {}", success_mark(), report.version);
    }
    say!("  → {}", report.install_dir.display());
    Ok(ExitCode::SUCCESS)
}

fn cmd_list(paths: &Paths, remote: bool) -> Result<ExitCode> {
    if remote {
        for v in ruby_install::list_remote()? {
            println!("{v}");
        }
    } else {
        let installed = resolve::list_installed(paths)?;
        if installed.is_empty() {
            println!("(no rubies installed; try `rv install 3.3.5`)");
        } else {
            for v in installed {
                println!("{v}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_current(paths: &Paths) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    match resolve::resolve(paths, &cwd)? {
        Some(r) => {
            println!("{}", r.version);
            let why = match r.source {
                VersionSource::EnvVar => "RV_VERSION".to_string(),
                VersionSource::Gemfile => format!(
                    "Gemfile ({})",
                    display_path(r.origin.as_deref().unwrap_or(Path::new("")))
                ),
                VersionSource::RubyVersionFile => format!(
                    ".ruby-version ({})",
                    display_path(r.origin.as_deref().unwrap_or(Path::new("")))
                ),
                VersionSource::Global => "global".to_string(),
                VersionSource::LatestInstalled => "latest installed".to_string(),
            };
            println!("  source: {why}");
            Ok(ExitCode::SUCCESS)
        }
        None => {
            println!("(no version resolved; run `rv install <version>`)");
            Ok(ExitCode::from(2))
        }
    }
}

fn cmd_which(paths: &Paths, name: &str) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    if let Some(bin) = lookup_project_tool(paths, &cwd, name)? {
        println!("{}", bin.display());
        return Ok(ExitCode::SUCCESS);
    }
    let r = resolve::resolve(paths, &cwd)?
        .ok_or_else(|| anyhow!("no Ruby resolved in {}", cwd.display()))?;
    let bin = paths.version_dir(&r.version).join("bin").join(name);
    if !bin.exists() {
        bail!(
            "{} not found in {}",
            name,
            paths.version_dir(&r.version).display()
        );
    }
    println!("{}", bin.display());
    Ok(ExitCode::SUCCESS)
}

fn cmd_use_global(paths: &Paths, version: &str) -> Result<ExitCode> {
    std::fs::write(paths.global_version_file(), version)
        .with_context(|| format!("write {}", paths.global_version_file().display()))?;
    println!("✓ global → {version}");
    Ok(ExitCode::SUCCESS)
}

fn cmd_run(paths: &Paths, argv: Vec<String>) -> Result<ExitCode> {
    if argv.is_empty() {
        bail!("usage: rv run <cmd> [args...]");
    }
    let cwd = std::env::current_dir()?;
    let cmd = &argv[0];
    let r = resolve::resolve(paths, &cwd)?
        .ok_or_else(|| anyhow!("no Ruby resolved in {}", cwd.display()))?;
    let ruby_dir = paths.version_dir(&r.version);
    let bin_dir = ruby_dir.join("bin");

    // First check the project's locked tools (per-tool gem_home).
    let exe: PathBuf = lookup_project_tool(paths, &cwd, cmd)?
        .or_else(|| {
            let candidate = bin_dir.join(cmd);
            if candidate.exists() {
                Some(candidate)
            } else {
                None
            }
        })
        .unwrap_or_else(|| PathBuf::from(cmd));

    use std::process::Command;
    let mut child = Command::new(&exe);
    child.args(&argv[1..]);
    // Stack tool gem-homes so `rubocop` can `require 'rubocop'` etc.
    let lock_root = project::find_root(&cwd);
    let gem_paths = collect_tool_gem_paths(paths, lock_root.as_deref(), &r.version);
    let mut path_var = std::ffi::OsString::from(bin_dir.as_os_str());
    for p in &gem_paths {
        path_var.push(":");
        path_var.push(p.join("bin"));
    }
    path_var.push(":");
    path_var.push(std::env::var_os("PATH").unwrap_or_default());
    child.env("PATH", path_var);
    if !gem_paths.is_empty() {
        let joined = gem_paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(":");
        child.env("GEM_PATH", joined);
    }
    let status = child
        .status()
        .with_context(|| format!("spawn {}", exe.display()))?;
    Ok(ExitCode::from(status.code().unwrap_or(1) as u8))
}

fn collect_tool_gem_paths(paths: &Paths, root: Option<&Path>, ruby_version: &str) -> Vec<PathBuf> {
    let Some(root) = root else {
        return Vec::new();
    };
    let lock = match Lock::load(root) {
        Ok(l) => l,
        Err(_) => return Vec::new(),
    };
    lock.tools
        .iter()
        .filter(|t| t.built_with == ruby_version)
        .map(|t| tool::tool_gem_home(paths, ruby_version, &t.gem, &t.version))
        .filter(|p| p.exists())
        .collect()
}

async fn cmd_add_tool(paths: &Paths, spec: &str) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let root = project::find_root(&cwd)
        .ok_or_else(|| anyhow!("no project root found above {}", cwd.display()))?;
    let (name, version) = parse_tool_spec(spec);
    let mut proj = project::load(&root)?;
    proj.tools.insert(
        name.clone(),
        ToolSpec::Short(version.unwrap_or_else(|| "latest".to_string())),
    );
    project::save(&root, &proj)?;
    println!("✓ pinned {name} in {}", root.join("rv.toml").display());
    sync_project(paths, &root, false).await?;
    Ok(ExitCode::SUCCESS)
}

async fn cmd_sync(paths: &Paths, frozen: bool) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let root = project::find_root(&cwd)
        .ok_or_else(|| anyhow!("no project root found above {}", cwd.display()))?;
    sync_project(paths, &root, frozen).await?;
    Ok(ExitCode::SUCCESS)
}

async fn sync_project(paths: &Paths, root: &Path, frozen: bool) -> Result<()> {
    let proj = project::load(root)?;
    let mut lock = Lock::load(root)?;

    let resolved = resolve::resolve(paths, root)?;
    let ruby_version = match resolved {
        Some(r) => r.version,
        None => proj.ruby.as_ref().map(|x| x.version.clone())
            .ok_or_else(|| anyhow!(
                "no Ruby version resolvable. Set `ruby` in Gemfile, write .ruby-version, or `[ruby]` in rv.toml"
            ))?,
    };
    if !paths
        .version_dir(&ruby_version)
        .join("bin")
        .join("ruby")
        .exists()
    {
        let pb = spinner(&format!(
            "installing ruby {ruby_version} for project (ruby-build)"
        ));
        ruby_install::install(paths, &ruby_version)?;
        pb.finish_and_clear();
    } else {
        say!(
            "{} ruby {ruby_version} {}",
            success_mark(),
            dim("(already present)")
        );
    }

    if proj.tools.is_empty() {
        if !frozen {
            let removed = lock.tools.len();
            lock.tools.clear();
            lock.ruby = Some(rv_core::lock::LockedRuby {
                version: ruby_version,
            });
            lock.save(root)?;
            if removed > 0 {
                println!(
                    " {} pruned {removed} stale lock entr{}",
                    dim("-"),
                    if removed == 1 { "y" } else { "ies" }
                );
            }
        }
        say!("{}", dim("(no tools to sync)"));
        return Ok(());
    }

    let client = http_client()?;
    let mp = MultiProgress::new();

    let resolve_started = Instant::now();
    let resolve_futs = proj.tools.iter().map(|(name, spec)| {
        let client = client.clone();
        let mp = mp.clone();
        let lock_ref = &lock;
        let name = name.clone();
        let spec = spec.clone();
        async move {
            let pb = mp.add(spinner(&format!("resolving {name}")));
            let r = if frozen {
                let l = lock_ref.find_tool(&name).ok_or_else(|| {
                    anyhow!("frozen sync: tool '{name}' is in rv.toml but not in rv.lock")
                })?;
                tool::ResolvedTool {
                    name: l.name.clone(),
                    gem: l.gem.clone(),
                    version: l.version.clone(),
                    bin: l.bin.clone(),
                    gem_sha256: l.gem_sha256.clone(),
                }
            } else {
                tool::resolve(&client, &name, &spec).await?
            };
            pb.finish_and_clear();
            Ok::<_, anyhow::Error>(r)
        }
    });
    let resolved_tools: Vec<tool::ResolvedTool> = try_join_all(resolve_futs).await?;
    say!(
        "{} Resolved {} tool{} in {}",
        success_mark(),
        resolved_tools.len(),
        plural(resolved_tools.len()),
        format_duration(resolve_started.elapsed().as_millis())
    );

    let install_started = Instant::now();
    let install_futs = resolved_tools.iter().map(|r| {
        let mp = mp.clone();
        let paths = paths.clone();
        let ruby_version = ruby_version.clone();
        let r = r.clone();
        async move {
            let pb = mp.add(spinner(&format!("installing {}@{}", r.name, r.version)));
            let res = tokio::task::spawn_blocking(move || tool::install(&paths, &ruby_version, &r))
                .await
                .map_err(|e| anyhow!("install task panicked: {e}"))??;
            pb.finish_and_clear();
            Ok::<_, anyhow::Error>(res)
        }
    });
    let installed: Vec<LockedTool> = try_join_all(install_futs).await?;
    say!(
        "{} Built {} tool{} in {}",
        success_mark(),
        installed.len(),
        plural(installed.len()),
        format_duration(install_started.elapsed().as_millis())
    );

    let mut summary: Vec<(String, String, char)> = Vec::with_capacity(installed.len());
    for new in installed {
        let mark = match lock.find_tool(&new.name).map(|l| l.version.clone()) {
            None => '+',
            Some(v) if v != new.version => '~',
            _ => '=',
        };
        summary.push((new.name.clone(), new.version.clone(), mark));
        lock.upsert_tool(new);
    }

    if !frozen {
        let known: std::collections::HashSet<&str> =
            proj.tools.keys().map(|s| s.as_str()).collect();
        let before = lock.tools.len();
        lock.tools.retain(|t| known.contains(t.name.as_str()));
        let removed = before - lock.tools.len();
        if removed > 0 {
            summary.push((
                format!(
                    "(pruned {removed} stale lock entr{})",
                    if removed == 1 { "y" } else { "ies" }
                ),
                String::new(),
                '-',
            ));
        }
    }
    summary.sort();
    for (name, version, mark) in &summary {
        let glyph = match mark {
            '+' => format!(" {}", color_green("+")),
            '~' => format!(" {}", color_yellow("~")),
            '-' => format!(" {}", dim("-")),
            _ => format!(" {}", dim("=")),
        };
        let detail = match mark {
            '+' => format!("{name}@{version} {}", dim("(new)")),
            '~' => format!("{name}@{version} {}", dim("(changed)")),
            '-' => name.clone(),
            _ => format!("{name}@{version} {}", dim("(unchanged)")),
        };
        println!("{glyph} {detail}");
    }

    lock.ruby = Some(rv_core::lock::LockedRuby {
        version: ruby_version,
    });
    if !frozen {
        lock.save(root)?;
    }
    Ok(())
}

fn cmd_init(with: Option<Vec<String>>, ruby: Option<String>, force: bool) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let target = cwd.join(project::PROJECT_FILE);
    if target.exists() && !force {
        bail!("{} already exists (use --force)", target.display());
    }
    let ruby_pin = ruby.or_else(|| {
        rv_core::manifest::find_project_version(&cwd)
            .ok()
            .flatten()
            .map(|h| h.version)
    });
    let mut proj = project::Project {
        ruby: ruby_pin.as_deref().map(|v| rv_core::project::RubySection {
            version: v.to_string(),
        }),
        tools: Default::default(),
    };
    if let Some(names) = with {
        for raw in names {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            let (name, version) = parse_tool_spec(raw);
            if version.is_none() && rv_core::registry::lookup(&name).is_none() {
                bail!("unknown tool '{name}' — pick from the registry or pass `name@version`");
            }
            proj.tools.insert(
                name,
                ToolSpec::Short(version.unwrap_or_else(|| "latest".to_string())),
            );
        }
    }
    project::save(&cwd, &proj)?;
    println!("{} wrote {}", success_mark(), target.display());
    if let Some(v) = ruby_pin {
        println!("    ruby      : {v}");
    }
    if proj.tools.is_empty() {
        println!(
            "    tools     : {} ({})",
            dim("(none)"),
            dim("add later via `rv add tool <name>`")
        );
    } else {
        println!("    tools     :");
        for (name, spec) in &proj.tools {
            println!("      - {name} = \"{}\"", spec.version());
        }
    }
    println!(
        "{}",
        dim("    next      : run `rv sync` to install everything")
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_tool_list(paths: &Paths) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let Some(root) = project::find_root(&cwd) else {
        bail!("no project root above {}", cwd.display());
    };
    let proj = project::load(&root)?;
    let lock = Lock::load(&root)?;
    if proj.tools.is_empty() {
        println!("{}", dim("(no tools pinned)"));
        return Ok(ExitCode::SUCCESS);
    }
    let w = proj.tools.keys().map(|s| s.len()).max().unwrap_or(0).max(4);
    println!(
        "{:<w$}  {:<12}  {:<10}  {}",
        color_bold("NAME"),
        color_bold("REQUESTED"),
        color_bold("LOCKED"),
        color_bold("STATUS"),
        w = w
    );
    for (name, spec) in &proj.tools {
        let req = spec.version();
        let (locked, status) = match lock.find_tool(name) {
            Some(t) => {
                let bin = tool::tool_bin_path(paths, t);
                let s = if bin.exists() {
                    color_green("present")
                } else {
                    color_yellow("missing")
                };
                (t.version.clone(), s)
            }
            None => ("—".into(), color_yellow("unsynced")),
        };
        println!(
            "{:<w$}  {:<12}  {:<10}  {}",
            name,
            req,
            locked,
            status,
            w = w
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_tool_registry() -> Result<ExitCode> {
    let entries = rv_core::registry::all();
    let w = entries
        .iter()
        .map(|e| e.name.len())
        .max()
        .unwrap_or(0)
        .max(4);
    println!(
        "{:<w$}  {:<24}  {}",
        color_bold("NAME"),
        color_bold("GEM"),
        color_bold("BIN"),
        w = w
    );
    for e in entries {
        println!("{:<w$}  {:<24}  {}", e.name, e.gem, e.bin, w = w);
    }
    println!();
    println!("{}", dim(&format!("    {} entries", entries.len())));
    Ok(ExitCode::SUCCESS)
}

fn cmd_tool_remove(paths: &Paths, name: &str) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let Some(root) = project::find_root(&cwd) else {
        bail!("no project root above {}", cwd.display());
    };
    let mut proj = project::load(&root)?;
    let mut lock = Lock::load(&root)?;
    let in_proj = proj.tools.remove(name).is_some();
    let before = lock.tools.len();
    lock.tools.retain(|t| t.name != name);
    let in_lock = before != lock.tools.len();
    if !in_proj && !in_lock {
        bail!("tool '{name}' is not pinned");
    }
    project::save(&root, &proj)?;
    lock.save(&root)?;
    println!(
        "{} removed {} from project",
        success_mark(),
        color_bold(name)
    );
    let _ = paths;
    println!(
        "{}",
        dim("    binary stays in the store; run `rv cache prune` to reclaim disk")
    );
    Ok(ExitCode::SUCCESS)
}

async fn cmd_x(paths: &Paths, argv: Vec<String>) -> Result<ExitCode> {
    if argv.is_empty() {
        bail!("usage: rvx <tool> [args...]");
    }
    let (spec, rest) = (&argv[0], &argv[1..]);
    let (name, version) = parse_tool_spec(spec);
    let spec_obj = ToolSpec::Short(version.unwrap_or_else(|| "latest".into()));
    let client = http_client()?;
    let pb = spinner(&format!("resolving {name}"));
    let resolved = tool::resolve(&client, &name, &spec_obj).await?;
    pb.finish_and_clear();

    let cwd = std::env::current_dir()?;
    let ruby_version = match resolve::resolve(paths, &cwd)? {
        Some(r) => r.version,
        None => {
            let installed = resolve::list_installed(paths)?;
            installed
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("no Ruby installed; run `rv install <version>` first"))?
        }
    };
    if !paths
        .version_dir(&ruby_version)
        .join("bin")
        .join("ruby")
        .exists()
    {
        let pb = spinner(&format!("installing ruby {ruby_version} for ephemeral run"));
        ruby_install::install(paths, &ruby_version)?;
        pb.finish_and_clear();
    }

    let bin_path = tool::tool_gem_home(paths, &ruby_version, &resolved.gem, &resolved.version)
        .join("bin")
        .join(&resolved.bin);
    if !bin_path.exists() {
        let pb = spinner(&format!(
            "installing {}@{}",
            resolved.name, resolved.version
        ));
        let r2 = resolved.clone();
        let p = paths.clone();
        let v = ruby_version.clone();
        tokio::task::spawn_blocking(move || tool::install(&p, &v, &r2))
            .await
            .map_err(|e| anyhow!("install task panicked: {e}"))??;
        pb.finish_and_clear();
        say!(
            "{} {} {}@{}",
            success_mark(),
            dim("ephemeral:"),
            resolved.name,
            resolved.version
        );
    }

    use std::process::Command;
    let bin_dir = paths.version_dir(&ruby_version).join("bin");
    let gem_home = tool::tool_gem_home(paths, &ruby_version, &resolved.gem, &resolved.version);
    let mut child = Command::new(&bin_path);
    child.args(rest);
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut new_path = std::ffi::OsString::from(bin_dir.as_os_str());
    new_path.push(":");
    new_path.push(&path);
    child.env("PATH", new_path);
    child.env("GEM_HOME", &gem_home);
    child.env("GEM_PATH", &gem_home);
    let status = child
        .status()
        .with_context(|| format!("spawn {}", bin_path.display()))?;
    Ok(ExitCode::from(status.code().unwrap_or(1) as u8))
}

async fn cmd_outdated(_paths: &Paths) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let Some(root) = project::find_root(&cwd) else {
        bail!("no project root above {}", cwd.display());
    };
    let proj = project::load(&root)?;
    let lock = Lock::load(&root)?;
    let client = http_client()?;
    let mut rows: Vec<(String, String, String, bool)> = Vec::new();

    if !proj.tools.is_empty() {
        let pb = spinner(&format!("checking {} tool(s)", proj.tools.len()));
        let futs = proj.tools.keys().map(|name| {
            let client = client.clone();
            let name = name.clone();
            async move {
                let r = tool::resolve(&client, &name, &ToolSpec::Short("latest".into())).await?;
                Ok::<_, anyhow::Error>((name, r.version))
            }
        });
        let resolved: Vec<(String, String)> = try_join_all(futs).await?;
        pb.finish_and_clear();
        for (name, latest) in resolved {
            let locked = lock
                .find_tool(&name)
                .map(|t| t.version.clone())
                .unwrap_or_else(|| "—".into());
            let behind = locked != latest;
            rows.push((name, locked, latest, behind));
        }
    }

    if rows.is_empty() {
        println!("{}", dim("(nothing to check)"));
        return Ok(ExitCode::SUCCESS);
    }
    let any_behind = rows.iter().any(|(_, _, _, b)| *b);
    let nw = rows.iter().map(|r| r.0.len()).max().unwrap_or(0).max(4);
    let cw = rows.iter().map(|r| r.1.len()).max().unwrap_or(0).max(6);
    let lw = rows.iter().map(|r| r.2.len()).max().unwrap_or(0).max(6);
    println!(
        "{:<nw$}  {:<cw$}  {:<lw$}  {}",
        color_bold("NAME"),
        color_bold("LOCKED"),
        color_bold("LATEST"),
        color_bold("STATUS"),
        nw = nw,
        cw = cw,
        lw = lw
    );
    for (name, locked, latest, behind) in &rows {
        let mark = if *behind {
            color_yellow("behind")
        } else {
            color_green("up to date")
        };
        println!(
            "{:<nw$}  {:<cw$}  {:<lw$}  {}",
            name,
            locked,
            latest,
            mark,
            nw = nw,
            cw = cw,
            lw = lw
        );
    }
    if any_behind {
        println!();
        println!("{} run `rv upgrade` to bump", dim("→"));
        Ok(ExitCode::from(2))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

async fn cmd_lock(paths: &Paths) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let root = project::find_root(&cwd)
        .ok_or_else(|| anyhow!("no project root above {}", cwd.display()))?;
    let proj = project::load(&root)?;
    let mut lock = Lock::load(&root)?;
    let client = http_client()?;
    let ruby_version = match resolve::resolve(paths, &root)? {
        Some(r) => r.version,
        None => proj
            .ruby
            .as_ref()
            .map(|x| x.version.clone())
            .ok_or_else(|| anyhow!("no Ruby resolvable"))?,
    };
    lock.ruby = Some(rv_core::lock::LockedRuby {
        version: ruby_version.clone(),
    });

    if !proj.tools.is_empty() {
        let pb = spinner(&format!("re-resolving {} tool(s)", proj.tools.len()));
        let futs = proj.tools.iter().map(|(name, spec)| {
            let client = client.clone();
            let name = name.clone();
            let spec = spec.clone();
            async move { tool::resolve(&client, &name, &spec).await }
        });
        let resolved: Vec<tool::ResolvedTool> = try_join_all(futs).await?;
        pb.finish_and_clear();
        for r in resolved {
            let prev = lock.find_tool(&r.name).cloned();
            let built_with = prev
                .as_ref()
                .filter(|p| p.version == r.version)
                .map(|p| p.built_with.clone())
                .unwrap_or_else(|| ruby_version.clone());
            lock.upsert_tool(LockedTool {
                name: r.name,
                gem: r.gem,
                version: r.version,
                bin: r.bin,
                gem_sha256: r.gem_sha256,
                built_with,
            });
        }
    }
    lock.save(&root)?;
    println!(
        "{} wrote {}",
        success_mark(),
        root.join("rv.lock").display()
    );
    say!(
        "{}",
        dim("    note: nothing was installed; run `rv sync` to materialize")
    );
    Ok(ExitCode::SUCCESS)
}

async fn cmd_upgrade(paths: &Paths, names: Vec<String>, upgrade_ruby: bool) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let root = project::find_root(&cwd)
        .ok_or_else(|| anyhow!("no project root above {}", cwd.display()))?;
    let proj = project::load(&root)?;
    let mut lock = Lock::load(&root)?;
    let client = http_client()?;

    let target_names: Vec<String> = if names.is_empty() {
        proj.tools.keys().cloned().collect()
    } else {
        for n in &names {
            if !proj.tools.contains_key(n) {
                bail!("tool '{n}' is not pinned");
            }
        }
        names
    };

    let ruby_version = match resolve::resolve(paths, &root)? {
        Some(r) => r.version,
        None => bail!("no Ruby resolved; run `rv sync` first"),
    };

    if upgrade_ruby {
        say!("{}", dim("(rv upgrade --ruby is not implemented yet for Ruby; pin via .ruby-version manually)"));
    }
    if target_names.is_empty() {
        lock.save(&root)?;
        return Ok(ExitCode::SUCCESS);
    }

    let mp = MultiProgress::new();
    let resolve_started = Instant::now();
    let resolve_futs = target_names.iter().map(|name| {
        let client = client.clone();
        let mp = mp.clone();
        let name = name.clone();
        async move {
            let pb = mp.add(spinner(&format!("resolving {name}@latest")));
            let r = tool::resolve(&client, &name, &ToolSpec::Short("latest".into())).await?;
            pb.finish_and_clear();
            Ok::<_, anyhow::Error>(r)
        }
    });
    let resolved: Vec<tool::ResolvedTool> = try_join_all(resolve_futs).await?;
    say!(
        "{} Resolved {} tool{} in {}",
        success_mark(),
        resolved.len(),
        plural(resolved.len()),
        format_duration(resolve_started.elapsed().as_millis())
    );

    let mut to_install: Vec<tool::ResolvedTool> = Vec::new();
    let mut bumps: Vec<(String, String, String)> = Vec::new();
    for r in resolved {
        match lock.find_tool(&r.name).map(|l| l.version.clone()) {
            Some(prev) if prev == r.version => {
                println!("  {} {} {}", dim("="), r.name, dim("(already latest)"))
            }
            prev => {
                bumps.push((
                    r.name.clone(),
                    prev.unwrap_or_else(|| "(none)".into()),
                    r.version.clone(),
                ));
                to_install.push(r);
            }
        }
    }
    if to_install.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }

    let install_started = Instant::now();
    let install_futs = to_install.iter().map(|r| {
        let mp = mp.clone();
        let paths = paths.clone();
        let ruby_version = ruby_version.clone();
        let r = r.clone();
        async move {
            let pb = mp.add(spinner(&format!("installing {}@{}", r.name, r.version)));
            let res = tokio::task::spawn_blocking(move || tool::install(&paths, &ruby_version, &r))
                .await
                .map_err(|e| anyhow!("install task panicked: {e}"))??;
            pb.finish_and_clear();
            Ok::<_, anyhow::Error>(res)
        }
    });
    let installed: Vec<LockedTool> = try_join_all(install_futs).await?;
    say!(
        "{} Built {} tool{} in {}",
        success_mark(),
        installed.len(),
        plural(installed.len()),
        format_duration(install_started.elapsed().as_millis())
    );
    for new in installed {
        lock.upsert_tool(new);
    }
    lock.save(&root)?;
    for (name, old, new) in &bumps {
        println!(
            " {} {name}: {} → {}",
            color_green("~"),
            dim(old),
            color_bold(new)
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_tree(paths: &Paths) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let root = project::find_root(&cwd);
    println!("{}", color_bold("rv tree"));
    let resolved = resolve::resolve(paths, &cwd)?;
    match resolved {
        Some(r) => {
            println!("├── {} {}", color_cyan("ruby"), color_bold(&r.version));
            println!("│   ├── source: {}", source_label(&r));
            println!(
                "│   └── home  : {}",
                paths.version_dir(&r.version).display()
            );
        }
        None => println!("├── {} {}", color_cyan("ruby"), dim("(none)")),
    }
    let lock = match root.as_deref() {
        Some(r) => Lock::load(r).unwrap_or_else(|_| Lock::empty()),
        None => Lock::empty(),
    };
    if lock.tools.is_empty() {
        println!("└── {} {}", color_cyan("tools"), dim("(none pinned)"));
    } else {
        println!("└── {} ({})", color_cyan("tools"), lock.tools.len());
        let last = lock.tools.len() - 1;
        for (i, t) in lock.tools.iter().enumerate() {
            let (br, ind) = if i == last {
                ("└──", "    ")
            } else {
                ("├──", "│   ")
            };
            let bin = tool::tool_bin_path(paths, t);
            let st = if bin.exists() {
                color_green("present")
            } else {
                color_yellow("missing")
            };
            println!("    {br} {} @ {}  [{}]", color_bold(&t.name), t.version, st);
            println!("    {ind}├── gem     : {}", t.gem);
            println!(
                "    {ind}├── sha     : {}",
                &t.gem_sha256.chars().take(20).collect::<String>()
            );
            println!("    {ind}├── built   : with {}", t.built_with);
            println!("    {ind}└── bin     : {}", bin.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn source_label(r: &resolve::Resolved) -> String {
    use VersionSource::*;
    match r.source {
        EnvVar => "RV_VERSION".into(),
        Gemfile => format!(
            "Gemfile ({})",
            display_path(r.origin.as_deref().unwrap_or(Path::new("")))
        ),
        RubyVersionFile => format!(
            ".ruby-version ({})",
            display_path(r.origin.as_deref().unwrap_or(Path::new("")))
        ),
        Global => "global".into(),
        LatestInstalled => "latest installed".into(),
    }
}

fn cmd_cache_info(paths: &Paths) -> Result<ExitCode> {
    let entries: Vec<(&str, PathBuf)> = vec![
        ("versions  ", paths.versions()),
        ("tools     ", paths.data.join("tools")),
        ("cache     ", paths.cache.clone()),
        ("config    ", paths.config.clone()),
    ];
    println!("{}", color_bold("rv cache"));
    let mut total = 0u64;
    for (label, path) in &entries {
        let (size, count) = if path.exists() {
            dir_size(path)?
        } else {
            (0, 0)
        };
        total += size;
        println!(
            "  {} {:>10}  {:>5} entr{}  {}",
            label,
            humanize(size),
            count,
            if count == 1 { "y" } else { "ies" },
            dim(&path.display().to_string())
        );
    }
    println!("  {} {:>10}", color_bold("total     "), humanize(total));
    Ok(ExitCode::SUCCESS)
}

fn cmd_cache_prune(paths: &Paths, dry_run: bool) -> Result<ExitCode> {
    // Ruby installs aren't symlinked into a separate store yet — they live
    // directly under versions/<v>. Pruning unused tools whose ruby is gone is
    // the most useful prune. We walk tools/<ruby_version>/* and drop anything
    // whose ruby_version isn't installed.
    let tools_dir = paths.data.join("tools");
    if !tools_dir.exists() {
        println!("{} nothing to prune", success_mark());
        return Ok(ExitCode::SUCCESS);
    }
    let mut to_remove: Vec<(PathBuf, u64)> = Vec::new();
    for entry in std::fs::read_dir(&tools_dir)? {
        let entry = entry?;
        let p = entry.path();
        let ruby_version = entry.file_name().to_string_lossy().to_string();
        if paths
            .version_dir(&ruby_version)
            .join("bin")
            .join("ruby")
            .exists()
        {
            continue;
        }
        let (sz, _) = dir_size(&p)?;
        to_remove.push((p, sz));
    }
    if to_remove.is_empty() {
        println!("{} nothing to prune", success_mark());
        return Ok(ExitCode::SUCCESS);
    }
    let total: u64 = to_remove.iter().map(|(_, s)| *s).sum();
    let verb = if dry_run { "would remove" } else { "removed" };
    for (p, sz) in &to_remove {
        println!("  {} {:>10}  {}", verb, humanize(*sz), p.display());
        if !dry_run {
            std::fs::remove_dir_all(p).with_context(|| format!("remove {}", p.display()))?;
        }
    }
    println!(
        "{} {} {} orphaned tool dir{} ({})",
        success_mark(),
        verb,
        to_remove.len(),
        if to_remove.len() == 1 { "" } else { "s" },
        humanize(total)
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_dir(paths: &Paths, kind: DirKind) -> Result<ExitCode> {
    let p = match kind {
        DirKind::Data => paths.data.clone(),
        DirKind::Cache => paths.cache.clone(),
        DirKind::Config => paths.config.clone(),
        DirKind::Versions => paths.versions(),
        DirKind::Tools => paths.data.join("tools"),
    };
    println!("{}", p.display());
    Ok(ExitCode::SUCCESS)
}

fn cmd_uninstall(paths: &Paths, version: &str) -> Result<ExitCode> {
    ruby_install::uninstall(paths, version)?;
    println!("{} uninstalled ruby {version}", success_mark());
    say!("{}", dim("    note: any per-tool gem-homes for this Ruby become orphaned; `rv cache prune` removes them"));
    Ok(ExitCode::SUCCESS)
}

fn cmd_env(paths: &Paths, shell: EnvShell) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let r = resolve::resolve(paths, &cwd)?
        .ok_or_else(|| anyhow!("no Ruby resolved in {}", cwd.display()))?;
    let ruby_dir = paths.version_dir(&r.version);
    let bin_dir = ruby_dir.join("bin");
    let lock_root = project::find_root(&cwd);
    let gem_paths = collect_tool_gem_paths(paths, lock_root.as_deref(), &r.version);
    let gem_path_joined = gem_paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(":");

    match shell {
        EnvShell::Sh => {
            println!(
                "export RUBY_ROOT={}",
                quote_sh(&ruby_dir.display().to_string())
            );
            if !gem_path_joined.is_empty() {
                println!("export GEM_PATH={}", quote_sh(&gem_path_joined));
            }
            println!(
                "export PATH={}:\"$PATH\"",
                quote_sh(&bin_dir.display().to_string())
            );
        }
        EnvShell::Fish => {
            println!(
                "set -gx RUBY_ROOT {}",
                quote_sh(&ruby_dir.display().to_string())
            );
            if !gem_path_joined.is_empty() {
                println!("set -gx GEM_PATH {}", quote_sh(&gem_path_joined));
            }
            println!(
                "set -gx PATH {} $PATH",
                quote_sh(&bin_dir.display().to_string())
            );
        }
        EnvShell::Powershell => {
            println!(
                "$env:RUBY_ROOT = {}",
                quote_ps(&ruby_dir.display().to_string())
            );
            if !gem_path_joined.is_empty() {
                println!("$env:GEM_PATH = {}", quote_ps(&gem_path_joined));
            }
            println!(
                "$env:Path = {} + ';' + $env:Path",
                quote_ps(&bin_dir.display().to_string())
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(serde::Deserialize)]
struct GhRelease {
    tag_name: String,
}

async fn cmd_self_update(check: bool) -> Result<ExitCode> {
    let current = env!("CARGO_PKG_VERSION");
    let client = http_client()?;
    let release: GhRelease = client
        .get("https://api.github.com/repos/O6lvl4/rv/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("parse GitHub release JSON")?;
    let latest_tag = release.tag_name;
    let latest = latest_tag.strip_prefix('v').unwrap_or(&latest_tag);
    if !is_semver_newer(latest, current) {
        println!(
            "{} rv is already up to date {}",
            success_mark(),
            dim(&format!("(installed: {current}, latest: {latest})"))
        );
        return Ok(ExitCode::SUCCESS);
    }
    if check {
        println!(
            "{} a newer release is available: {} {} {}",
            success_mark(),
            dim(current),
            dim("→"),
            color_bold(latest)
        );
        return Ok(ExitCode::SUCCESS);
    }
    let triple =
        target_triple().ok_or_else(|| anyhow!("self-update unsupported on this platform"))?;
    let asset = format!("rv-{latest_tag}-{triple}.tar.gz");
    let url = format!("https://github.com/O6lvl4/rv/releases/download/{latest_tag}/{asset}");
    let pb = spinner(&format!("downloading {asset}"));
    let bytes = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let sha_text = client
        .get(format!("{url}.sha256"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    pb.finish_and_clear();
    let expected: String = sha_text.split_whitespace().next().unwrap_or("").to_string();
    let mut hasher = sha2::Sha256::new();
    hasher.update(&bytes);
    let actual = hex::encode(hasher.finalize());
    if !expected.is_empty() && expected != actual {
        bail!("sha256 mismatch: expected {expected}, got {actual}");
    }
    let tmp = tempdir("rv-self-update-")?;
    let archive_path = tmp.join(&asset);
    std::fs::write(&archive_path, &bytes)?;
    let f = std::fs::File::open(&archive_path)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(f));
    tar.unpack(&tmp)?;
    let stage = tmp.join(format!("rv-{latest_tag}-{triple}"));
    let new_rv = stage.join("rv");
    let current_exe = std::env::current_exe()?;
    std::fs::rename(&new_rv, &current_exe)
        .or_else(|_| {
            std::fs::copy(&new_rv, &current_exe)
                .map(|_| ())
                .and_then(|_| std::fs::remove_file(&new_rv))
        })
        .with_context(|| format!("install binary at {}", current_exe.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&current_exe)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&current_exe, perms)?;
    }
    println!(
        "{} rv {} → {}",
        success_mark(),
        dim(current),
        color_bold(latest)
    );
    Ok(ExitCode::SUCCESS)
}

fn target_triple() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        _ => return None,
    })
}
fn is_semver_newer(latest: &str, current: &str) -> bool {
    fn parse(s: &str) -> (u64, u64, u64) {
        let mut p = s.split('.').map(|x| x.split('-').next().unwrap_or(""));
        (
            p.next().and_then(|x| x.parse().ok()).unwrap_or(0),
            p.next().and_then(|x| x.parse().ok()).unwrap_or(0),
            p.next().and_then(|x| x.parse().ok()).unwrap_or(0),
        )
    }
    parse(latest) > parse(current)
}
fn tempdir(prefix: &str) -> Result<PathBuf> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let p = std::env::temp_dir().join(format!("{prefix}{nonce}"));
    std::fs::create_dir_all(&p)?;
    Ok(p)
}

fn cmd_completions(shell: clap_complete::Shell) -> Result<ExitCode> {
    let mut cmd = <Cli as clap::CommandFactory>::command();
    let bin = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, bin, &mut std::io::stdout());
    Ok(ExitCode::SUCCESS)
}

fn cmd_doctor(paths: &Paths) -> Result<ExitCode> {
    println!("rv doctor");
    println!("  data dir   : {}", paths.data.display());
    println!("  config dir : {}", paths.config.display());
    println!("  cache dir  : {}", paths.cache.display());
    let installed = resolve::list_installed(paths)?;
    println!("  installed  : {} ruby version(s)", installed.len());
    for v in installed.iter().take(8) {
        println!("    - {v}");
    }
    let cwd = std::env::current_dir()?;
    match resolve::resolve(paths, &cwd)? {
        Some(r) => println!("  resolved   : {} (from {:?})", r.version, r.source),
        None => println!("  resolved   : (none)"),
    }
    let rb = std::process::Command::new("ruby-build")
        .arg("--version")
        .output();
    match rb {
        Ok(o) if o.status.success() => println!(
            "  ruby-build : {}",
            String::from_utf8_lossy(&o.stdout).trim()
        ),
        _ => println!("  ruby-build : MISSING — install with `brew install ruby-build`"),
    }
    Ok(ExitCode::SUCCESS)
}

// ----- presentation helpers --------------------------------------------------

fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("rv/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

fn spinner(msg: &str) -> ProgressBar {
    if quiet() {
        return ProgressBar::hidden();
    }
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("  {spinner:.green} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

fn success_mark() -> &'static str {
    "\x1b[32m✓\x1b[0m"
}
fn dim(s: &str) -> String {
    format!("\x1b[2m{s}\x1b[0m")
}
fn color_green(s: &str) -> String {
    format!("\x1b[32m{s}\x1b[0m")
}
fn color_yellow(s: &str) -> String {
    format!("\x1b[33m{s}\x1b[0m")
}
fn color_cyan(s: &str) -> String {
    format!("\x1b[36m{s}\x1b[0m")
}
fn color_bold(s: &str) -> String {
    format!("\x1b[1m{s}\x1b[0m")
}
fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}
fn format_duration(ms: u128) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.2}s", ms as f64 / 1_000.0)
    } else {
        let s = ms / 1_000;
        format!("{}m{:02}s", s / 60, s % 60)
    }
}
fn humanize(bytes: u64) -> String {
    const U: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes == 0 {
        return "0 B".into();
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 || i == 0 {
        format!("{:.0} {}", v, U[i])
    } else {
        format!("{:.1} {}", v, U[i])
    }
}
fn quote_sh(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
fn quote_ps(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
fn parse_tool_spec(s: &str) -> (String, Option<String>) {
    if let Some((n, v)) = s.rsplit_once('@') {
        (n.to_string(), Some(v.to_string()))
    } else {
        (s.to_string(), None)
    }
}
fn display_path(p: &Path) -> String {
    p.display().to_string()
}

fn lookup_project_tool(paths: &Paths, cwd: &Path, name: &str) -> Result<Option<PathBuf>> {
    let Some(root) = project::find_root(cwd) else {
        return Ok(None);
    };
    let lock = Lock::load(&root)?;
    let Some(t) = lock.find_tool(name) else {
        return Ok(None);
    };
    let bin = tool::tool_bin_path(paths, t);
    Ok(if bin.exists() { Some(bin) } else { None })
}

fn dir_size(path: &Path) -> Result<(u64, usize)> {
    if path.is_file() {
        return Ok((std::fs::metadata(path)?.len(), 1));
    }
    if !path.is_dir() {
        return Ok((0, 0));
    }
    let mut total = 0u64;
    let mut count = 0usize;
    let mut stack = vec![path.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let p = entry.path();
            let meta = entry.metadata()?;
            if meta.is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if d == path {
                    count += 1;
                }
                stack.push(p);
            } else {
                if d == path {
                    count += 1;
                }
                total += meta.len();
            }
        }
    }
    Ok((total, count))
}
