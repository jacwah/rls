// Copyright 2016 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

extern crate racer;
extern crate rustfmt;

use analysis::{AnalysisHost, Span};
use self::racer::core::complete_from_file;
use self::racer::core::find_definition;
use self::racer::core;
use self::racer::scopes;
use self::rustfmt::{Input as FmtInput, format_input};
use self::rustfmt::config::{self, WriteMode};

use std::default::Default;
use std::fs::File;
use std::io::prelude::*;
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use actions_common::*;

use ide::{self, Input, Output, FmtOutput, VscodeKind};
use vfs::Vfs;

// Timeout = 0.5s (totally arbitrary).
const RUSTW_TIMEOUT: u64 = 500;

pub fn complete(source: Position, _analysis: Arc<AnalysisHost>) -> Vec<Completion> {
    panic::catch_unwind(|| {
        // TODO RacerUp
        let source = adjust_vscode_pos_for_racer(source);
        let path = Path::new(&source.filepath);
        let mut f = File::open(&path).unwrap();
        let mut src = String::new();
        f.read_to_string(&mut src).unwrap();
        let cache = core::FileCache::new();
        let session = core::Session::from_path(&cache, &path, &path);
        let pos = session.load_file(&path).coords_to_point(source.line, source.col).unwrap();
        let got = complete_from_file(&src, &path, pos, &session);

        let mut results = vec![];
        for comp in got {
            results.push(Completion {
                name: comp.matchstr.clone(),
                context: comp.contextstr.clone(),
            });
        }
        results
    }).unwrap_or(vec![])
}

pub fn find_refs(source: Input, analysis: Arc<AnalysisHost>) -> Vec<Span> {
    let t = thread::current();
    let span = source.span;
    println!("title for: {:?}", span);
    let rustw_handle = thread::spawn(move || {
        let result = analysis.find_all_refs(&span);
        t.unpark();

        println!("rustw find_all_refs: {:?}", result);
        result
    });

    thread::park_timeout(Duration::from_millis(RUSTW_TIMEOUT));

    rustw_handle.join().ok().and_then(|t| t.ok()).unwrap_or(vec![])
}

pub fn fmt(file_name: &str, vfs: Arc<Vfs>) -> FmtOutput {
    let path = PathBuf::from(file_name);
    let input = match vfs.get_file_changes(&path) {
        Some(s) => FmtInput::Text(s),
        None => FmtInput::File(path),
    };

    let mut config = config::Config::default();
    config.skip_children = true;
    config.write_mode = WriteMode::Plain;

    let mut buf = Vec::<u8>::new();
    match format_input(input, &config, Some(&mut buf)) {
        Ok(_) => FmtOutput::Change(String::from_utf8(buf).unwrap()),
        Err(_) => FmtOutput::Err,
    }
}

pub fn goto_def(source: Input, analysis: Arc<AnalysisHost>) -> Output {
    // Rustw thread.
    let t = thread::current();
    let span = source.span;
    let rustw_handle = thread::spawn(move || {
        let result = if let Ok(s) = analysis.goto_def(&span) {
            println!("rustw success!");
            Some(Position {
                filepath: s.file_name,
                line: s.line_start,
                col: s.column_start,
            })
        } else {
            println!("rustw failed");
            None
        };

        t.unpark();

        result
    });

    // Racer thread.
    let pos = adjust_vscode_pos_for_racer(source.pos);
    let racer_handle = thread::spawn(move || {
        // FIXME(#23) RacerUp
        // let path = Path::new(&pos.filepath);
        // let mut f = File::open(&path).unwrap();
        // let mut src = String::new();
        // f.read_to_string(&mut src).unwrap();
        // let pos = scopes::coords_to_point(&src, pos.line, pos.col);
        // let cache = core::FileCache::new();
        // if let Some(mch) = find_definition(&src,
        //                                    &path,
        //                                    pos,
        //                                    &core::Session::from_path(&cache, &path, &path)) {
        //     let mut f = File::open(&mch.filepath).unwrap();
        //     let mut source_src = String::new();
        //     f.read_to_string(&mut source_src).unwrap();
        //     if mch.point != 0 {
        //         let (line, col) = scopes::point_to_coords(&source_src, mch.point);
        //         let fpath = mch.filepath.to_str().unwrap().to_string();
        //         Some(Position {
        //             filepath: fpath,
        //             line: line,
        //             col: col,
        //         })
        //     } else {
        //         None
        //     }
        // } else {
        //     None
        // }

        None
    });

    thread::park_timeout(Duration::from_millis(RUSTW_TIMEOUT));

    let rustw_result = rustw_handle.join().unwrap_or(None);
    match rustw_result {
        Some(r) => {
            Output::Ok(r, Provider::Compiler)
        }
        None => {
            println!("Using racer");
            match racer_handle.join() {
                Ok(Some(r)) => {
                    Output::Ok(adjust_racer_pos_for_vscode(r), Provider::Racer)
                }
                _ => Output::Err,
            }
        }
    }
}

pub fn title(source: Input, analysis: Arc<AnalysisHost>) -> Option<Title> {
    let t = thread::current();
    let span = source.span;
    println!("title for: {:?}", span);
    let rustw_handle = thread::spawn(move || {
        let ty = analysis.show_type(&span).unwrap_or(String::new());
        let docs = analysis.docs(&span).unwrap_or(String::new());
        let doc_url = analysis.doc_url(&span).unwrap_or(String::new());
        t.unpark();

        println!("rustw show_type: {:?}", ty);
        println!("rustw docs: {:?}", docs);
        println!("rustw doc url: {:?}", doc_url);
        Title {
            ty: ty,
            docs: docs,
            doc_url: doc_url,
        }
    });

    thread::park_timeout(Duration::from_millis(RUSTW_TIMEOUT));

    rustw_handle.join().ok()
}

pub fn symbols(file_name: String, analysis: Arc<AnalysisHost>) -> Vec<Symbol> {
    let t = thread::current();
    let rustw_handle = thread::spawn(move || {
        let symbols = analysis.symbols(&file_name).unwrap_or(vec![]);
        t.unpark();

        symbols.into_iter().map(|s| {
            Symbol {
                name: s.name,
                kind: VscodeKind::from(s.kind),
                span: s.span,
            }
        }).collect()
    });

    thread::park_timeout(Duration::from_millis(RUSTW_TIMEOUT));

    rustw_handle.join().unwrap_or(vec![])
}


fn adjust_vscode_pos_for_racer(mut source: Position) -> Position {
    source.line += 1;
    source
}

fn adjust_racer_pos_for_vscode(mut source: Position) -> Position {
    if source.line > 0 {
        source.line -= 1;
    }
    source
}