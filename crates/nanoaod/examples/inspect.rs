//! Prints what is inside a ROOT file: its objects, and for a tree its branches
//! and a checksum of every column.
//!
//! Useful for finding your way around a NanoAOD file before writing against
//! it, and for checking that two files agree bin for bin and row for row.
//!
//! ```sh
//! cargo run --example inspect -- fourMuonMass.root
//! cargo run --example inspect -- root://cms-xrd-global.cern.ch//store/mc/….root Events
//! ```

use std::process::ExitCode;

use nanoaod::prelude::*;
use root_io::tree::{LeafKind, Tree};
use root_io::Element;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("usage: inspect <file.root|root://…> [tree]");
        return ExitCode::FAILURE;
    };
    if let Err(e) = inspect(path, args.get(2).map(String::as_str)) {
        eprintln!("inspect: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn inspect(path: &str, tree_name: Option<&str>) -> Result<()> {
    let mut file = RootFile::open(path)?;
    println!("file    {}", file.describe());
    println!("size    {} bytes", file.size());

    let keys: Vec<(String, String)> = file
        .keys()
        .iter()
        .map(|k| (k.name.clone(), k.class_name.clone()))
        .collect();

    println!("\nobjects");
    for (name, class) in &keys {
        println!("  {name:<24} {class}");
    }

    for (name, class) in &keys {
        if class.starts_with("TH1") || class.starts_with("TH2") {
            let h = file.histogram(name)?;
            println!("\nhistogram {name} ({class})");
            println!("  entries  {}", h.entries());
            println!("  integral {}", h.integral());
            println!("  mean     {:.9}", h.mean());
            println!("  std dev  {:.9}", h.std_dev());
            let contents: f64 = (0..h.ncells()).map(|b| h.bin_content(b)).sum();
            let errors: f64 = (0..h.ncells()).map(|b| h.bin_error(b)).sum();
            println!(
                "  cells    {} sum={contents} errsum={errors:.9}",
                h.ncells()
            );
        }
    }

    // Trees: the one asked for, else every tree in the file.
    let trees: Vec<&str> = match tree_name {
        Some(t) => vec![t],
        None => keys
            .iter()
            .filter(|(_, class)| class == "TTree")
            .map(|(name, _)| name.as_str())
            .collect(),
    };

    for name in trees {
        let tree = file.tree(name)?;
        println!("\ntree {} — {} entries", tree.name, tree.entries);
        for branch in &tree.branches {
            let leaf = branch.leaf()?;
            let shape = if leaf.has_count {
                "[]".to_string()
            } else if leaf.len > 1 {
                format!("[{}]", leaf.len)
            } else {
                String::new()
            };
            let digest = column_digest(&mut file, &tree, &branch.name, leaf.kind)?;
            println!(
                "  {:<20} {}{:<4} {}",
                branch.name,
                leaf.kind.type_name(),
                shape,
                digest
            );
        }
    }
    Ok(())
}

/// A sum over a branch's values, which is enough to tell two files apart
/// without printing every number.
fn column_digest(file: &mut RootFile, tree: &Tree, branch: &str, kind: LeafKind) -> Result<String> {
    fn sum<T: Element + Into<f64>>(
        file: &mut RootFile,
        tree: &Tree,
        branch: &str,
    ) -> Result<String> {
        let column = file.jagged::<T>(tree, branch)?;
        let values = column.values();
        let total: f64 = values.iter().copied().map(Into::into).sum();
        Ok(format!("n={} sum={total:.6}", values.len()))
    }

    match kind {
        LeafKind::Bool => {
            let column = file.jagged::<bool>(tree, branch)?;
            let set = column.values().iter().filter(|&&b| b).count();
            Ok(format!("n={} true={set}", column.values().len()))
        }
        LeafKind::I8 => sum::<i8>(file, tree, branch),
        LeafKind::U8 => sum::<u8>(file, tree, branch),
        LeafKind::I16 => sum::<i16>(file, tree, branch),
        LeafKind::U16 => sum::<u16>(file, tree, branch),
        LeafKind::I32 => sum::<i32>(file, tree, branch),
        LeafKind::U32 => sum::<u32>(file, tree, branch),
        LeafKind::F32 => sum::<f32>(file, tree, branch),
        LeafKind::F64 => sum::<f64>(file, tree, branch),
        // 64-bit integers do not convert to f64 without loss, so they are
        // folded rather than summed.
        LeafKind::I64 | LeafKind::U64 => {
            let column = file.jagged::<i64>(tree, branch)?;
            let folded = column
                .values()
                .iter()
                .fold(0i64, |acc, &v| acc.wrapping_mul(31).wrapping_add(v));
            Ok(format!("n={} fold={folded}", column.values().len()))
        }
    }
}
