//! Scanner performance benchmark commands and regression policy.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use anyhow::{Result, bail};
use rayon::prelude::*;

use crate::config::{Language, ProjectConfig};
use crate::diagnostic::DiagnosticResultExt;
use crate::diagnostic::Diagnostics;
use crate::extract::build_catalog;

pub fn run(
    config: &ProjectConfig,
    iterations: usize,
    synthetic_mib: usize,
    baseline_mib_s: Option<f64>,
) -> Result<()> {
    run_impl(config, iterations, synthetic_mib, baseline_mib_s).diagnostic(
        "trox.benchmark",
        Some(config.path.clone()),
        "Correct the benchmark corpus or investigate the reported performance regression.",
    )
}

fn run_impl(
    config: &ProjectConfig,
    iterations: usize,
    synthetic_mib: usize,
    baseline_mib_s: Option<f64>,
) -> Result<()> {
    if iterations == 0 {
        bail!("iterations must be positive");
    }
    if baseline_mib_s.is_some_and(|baseline| !baseline.is_finite() || baseline <= 0.0) {
        bail!("baseline_mib_s must be a positive finite number");
    }
    if synthetic_mib > 0 {
        return synthetic(iterations, synthetic_mib, baseline_mib_s);
    }
    let mut durations = Vec::new();
    let mut bytes = 0;
    let mut files = 0;
    let mut messages = 0;
    for _ in 0..iterations {
        let mut diagnostics = Diagnostics::default();
        let start = Instant::now();
        let model = build_catalog(config, &mut diagnostics)?;
        durations.push(start.elapsed());
        bytes = model.bytes_scanned;
        files = model.files_scanned;
        messages = model.messages.len();
        if diagnostics.has_errors() {
            bail!("benchmark corpus contains extraction errors");
        }
    }
    durations.sort();
    let median = durations[durations.len() / 2];
    let throughput = bytes as f64 / median.as_secs_f64() / (1024.0 * 1024.0);
    println!(
        "files={files} bytes={bytes} messages={messages} iterations={iterations} median_ms={:.3} throughput_mib_s={throughput:.2}",
        median.as_secs_f64() * 1000.0
    );
    enforce_baseline(throughput, baseline_mib_s)
}

fn synthetic(iterations: usize, total_mib: usize, baseline_mib_s: Option<f64>) -> Result<()> {
    if total_mib < 4 {
        bail!("synthetic_mib must be at least 4");
    }
    let directory = tempfile::tempdir()?;
    let per_file = total_mib * 1024 * 1024 / 4;
    let specifications = [
        ("bench.rs", Language::Rust),
        ("bench.ts", Language::TypeScript),
        ("bench.tsx", Language::TypeScript),
        ("bench.ron", Language::Ron),
    ];
    let calls_per_file = 2_500;
    for (name, language) in specifications {
        generate_file(
            &directory.path().join(name),
            language,
            per_file,
            calls_per_file,
        )?;
    }
    let files: Vec<_> = specifications
        .into_iter()
        .map(|(name, language)| (directory.path().join(name), language))
        .collect();
    let scan = || -> Result<(u64, usize)> {
        let results: Vec<_> = files
            .par_iter()
            .map(|(path, language)| {
                crate::scanner::scan_file(path, *language, Some("Benchmark RON string."))
            })
            .collect();
        if let Some(error) = results
            .iter()
            .flat_map(|result| result.diagnostics.iter())
            .next()
        {
            bail!("synthetic corpus extraction failed: {error}");
        }
        Ok((
            results.iter().map(|result| result.bytes_scanned).sum(),
            results.iter().map(|result| result.messages.len()).sum(),
        ))
    };
    let cold_start = Instant::now();
    let (bytes, calls) = scan()?;
    let cold = cold_start.elapsed();
    let mut warm = Vec::new();
    for _ in 0..iterations {
        let start = Instant::now();
        let measured = scan()?;
        if measured != (bytes, calls) {
            bail!("nondeterministic benchmark scan counts");
        }
        warm.push(start.elapsed());
    }
    warm.sort();
    let median = warm[warm.len() / 2];
    let throughput = bytes as f64 / median.as_secs_f64() / (1024.0 * 1024.0);
    let peak = peak_rss_bytes();
    println!(
        "corpus_mib={:.2} files=4 calls={calls} cold_ms={:.3} warm_median_ms={:.3} throughput_mib_s={throughput:.2} peak_rss_bytes={peak}",
        bytes as f64 / 1024.0 / 1024.0,
        cold.as_secs_f64() * 1000.0,
        median.as_secs_f64() * 1000.0
    );
    if calls != 10_000 {
        bail!("normative benchmark corpus must contain 10,000 calls, found {calls}");
    }
    if bytes == 100 * 1024 * 1024 {
        if throughput < 200.0 {
            bail!("normative warm throughput {throughput:.2} MiB/s is below 200 MiB/s");
        }
        if median >= std::time::Duration::from_millis(50) {
            bail!(
                "normative warm median {:.3} ms is not below 50 ms",
                median.as_secs_f64() * 1000.0
            );
        }
        if peak > bytes * 2 {
            bail!("normative peak RSS {peak} exceeds twice the source bytes");
        }
    }
    enforce_baseline(throughput, baseline_mib_s)
}

fn enforce_baseline(throughput: f64, baseline_mib_s: Option<f64>) -> Result<()> {
    if let Some(baseline) = baseline_mib_s
        && throughput < baseline * 0.85
    {
        bail!(
            "median throughput {throughput:.2} MiB/s regressed more than 15% from {baseline:.2} MiB/s"
        );
    }
    Ok(())
}

fn generate_file(path: &Path, language: Language, target_bytes: usize, calls: usize) -> Result<()> {
    let mut file = File::create(path)?;
    let tsx = path.extension().and_then(|extension| extension.to_str()) == Some("tsx");
    let filler = match language {
        Language::Ron => "// Irrelevant authored-data filler with tx( and Tx lookalikes.\n",
        _ => "// Irrelevant source filler with tx( and Tx lookalikes.\n",
    };
    let filler_per_call = target_bytes.saturating_sub(calls * 160) / calls;
    for index in 0..calls {
        let call = match language {
            Language::Rust if index % 20 == 0 => format!(
                "fn b{index}(n:u32,owner:&str){{txa(select(owner,[when(\"player\",plural(n,[one(\"{{n}} nested item {index}\"),other(\"{{n}} nested items {index}\")])),otherwise(\"No nested items {index}\")]),tx_args![n],\"Synthetic nested benchmark message.\");}}\n"
            ),
            Language::Rust if index % 10 == 0 => format!(
                "fn b{index}(n:u32){{txa(plural(n,[one(\"{{n}} benchmark item {index}\"),other(\"{{n}} benchmark items {index}\")]),tx_args![n],\"Synthetic benchmark cardinal message.\");}}\n"
            ),
            Language::Rust => format!(
                "fn b{index}(){{tx(\"Benchmark Rust label {index}\",\"Synthetic benchmark static label.\");}}\n"
            ),
            Language::TypeScript if tsx && index % 20 == 0 => format!(
                "export const B{index}=({{n,owner}}:{{n:number;owner:string}})=><span title={{txa(select(owner,[when(`player`,plural(n,[one(`{{n}} nested item {index}`),other(`{{n}} nested items {index}`)])),otherwise(`No nested items {index}`)]),{{n}},`Synthetic nested TSX benchmark message.`)}}>benchmark</span>;\n"
            ),
            Language::TypeScript if tsx && index % 10 == 0 => format!(
                "export const B{index}=({{n}}:{{n:number}})=><span title={{txa(plural(n,[one(`{{n}} benchmark item {index}`),other(`{{n}} benchmark items {index}`)]),{{n}},`Synthetic benchmark cardinal message.`)}}>benchmark</span>;\n"
            ),
            Language::TypeScript if tsx => format!(
                "export const B{index}=()=> <button aria-label={{tx(`Benchmark TSX label {index}`,`Synthetic benchmark static label.`)}}>benchmark</button>;\n"
            ),
            Language::TypeScript if index % 20 == 0 => format!(
                "export const b{index}=(n:number,owner:string)=>txa(select(owner,[when(`player`,plural(n,[one(`{{n}} nested item {index}`),other(`{{n}} nested items {index}`)])),otherwise(`No nested items {index}`)]),{{n}},`Synthetic nested benchmark message.`);\n"
            ),
            Language::TypeScript if index % 10 == 0 => format!(
                "export const b{index}=(n:number)=>txa(plural(n,[one(`{{n}} benchmark item {index}`),other(`{{n}} benchmark items {index}`)]),{{n}},`Synthetic benchmark cardinal message.`);\n"
            ),
            Language::TypeScript => format!(
                "export const b{index}=()=>tx(`Benchmark TypeScript label {index}`,`Synthetic benchmark static label.`);\n"
            ),
            Language::Ron => format!(
                "(internal_id:\"bench-{index}\",label:Tx(text:\"Benchmark RON label {index}\")),\n"
            ),
        };
        file.write_all(call.as_bytes())?;
        let repeat = filler_per_call / filler.len();
        for _ in 0..repeat {
            file.write_all(filler.as_bytes())?;
        }
    }
    let current = file.metadata()?.len() as usize;
    if current < target_bytes {
        let remainder = target_bytes - current;
        let mut tail = String::from("//");
        tail.extend(std::iter::repeat_n('x', remainder.saturating_sub(3)));
        tail.push('\n');
        file.write_all(tail.as_bytes())?;
    }
    Ok(())
}

#[cfg(unix)]
fn peak_rss_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the provided rusage structure on success.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return 0;
    }
    // SAFETY: the successful call above initialized `usage`.
    let rss = unsafe { usage.assume_init() }.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        rss
    } else {
        rss * 1024
    }
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_policy_rejects_regressions_in_every_benchmark_mode() {
        assert!(enforce_baseline(84.99, Some(100.0)).is_err());
        assert!(enforce_baseline(85.0, Some(100.0)).is_ok());
        assert!(enforce_baseline(0.0, None).is_ok());
    }

    #[test]
    fn synthetic_corpus_exercises_nested_patterns_and_real_tsx() {
        let directory = tempfile::tempdir().unwrap();
        let rust = directory.path().join("bench.rs");
        let tsx = directory.path().join("bench.tsx");
        generate_file(&rust, Language::Rust, 32_000, 20).unwrap();
        generate_file(&tsx, Language::TypeScript, 32_000, 20).unwrap();

        let rust_result = crate::scanner::scan_file(&rust, Language::Rust, None);
        let tsx_result = crate::scanner::scan_file(&tsx, Language::TypeScript, None);
        assert!(rust_result.diagnostics.is_empty());
        assert!(tsx_result.diagnostics.is_empty());
        assert!(
            rust_result
                .messages
                .iter()
                .any(|message| message.selector_labels.len() == 2)
        );
        assert!(
            tsx_result
                .messages
                .iter()
                .any(|message| message.selector_labels.len() == 2)
        );
        assert!(std::fs::read_to_string(tsx).unwrap().contains("<button"));
    }
}
