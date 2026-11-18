//! `enso-files --bench list FOLDER`: how long a folder takes, step by step,
//! measured the way the window does it — so a regression shows from a
//! terminal, without a window.

use std::{
    path::Path,
    process::ExitCode,
    sync::{Mutex, atomic::AtomicBool},
    time::{Duration, Instant},
};

use enso_fs::{
    list::{self, Listing},
    sort::{self, By, Keys, Sort},
};

/// Runs of each step; the best is reported, as the least disturbed.
const RUNS: usize = 5;

pub fn list(folder: &Path) -> ExitCode {
    let first = Instant::now();
    let listing = match Listing::read(folder) {
        Ok(listing) => listing,
        Err(err) => {
            eprintln!("enso-files: cannot list {}: {err}", folder.display());
            return ExitCode::FAILURE;
        }
    };
    let cold = first.elapsed();
    println!("{}: {} entries", folder.display(), listing.len());
    println!("  first read (perhaps cold)  {}", ms(cold));

    let names = best(|| {
        let _ = Listing::read(folder);
    });
    println!("  names and kinds            {}", ms(names));

    let threads = list::threads();
    let mut statted = listing.clone();
    let stats = best(|| {
        let batches = Mutex::new(Vec::new());
        let _ = list::stat(
            &listing,
            threads,
            &AtomicBool::new(false),
            |start, metas| {
                if let Ok(mut batches) = batches.lock() {
                    batches.push((start, metas));
                }
            },
        );
        if let Ok(batches) = batches.into_inner() {
            for (start, metas) in batches {
                statted.apply(start, &metas);
            }
        }
    });
    println!("  metadata, {threads} threads        {}", ms(stats));

    let keys = best(|| {
        let _ = Keys::new(&statted);
    });
    println!("  sort keys                  {}", ms(keys));
    let folded = Keys::new(&statted);
    for by in [By::Name, By::Size, By::Modified] {
        let sort = Sort {
            by,
            ..Sort::default()
        };
        let took = best(|| {
            let _ = sort::order(&statted, &folded, sort);
        });
        println!("  sort by {:<18} {}", by.name(), ms(took));
    }
    println!(
        "  first frame needs          {}  (names, keys, sort by name)",
        ms(names
            + keys
            + best(|| {
                let _ = sort::order(&statted, &folded, Sort::default());
            }))
    );
    ExitCode::SUCCESS
}

fn best(mut step: impl FnMut()) -> Duration {
    (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            step();
            start.elapsed()
        })
        .min()
        .unwrap_or_default()
}

fn ms(duration: Duration) -> String {
    format!("{:>8.2} ms", duration.as_secs_f64() * 1000.0)
}
