//       ___           ___           ___           ___
//      /\__\         /\  \         /\  \         /\__\
//     /:/  /         \:\  \        \:\  \       /::|  |
//    /:/__/           \:\  \        \:\  \     /:|:|  |
//   /::\  \ ___       /::\  \       /::\  \   /:/|:|__|__
//  /:/\:\  /\__\     /:/\:\__\     /:/\:\__\ /:/ |::::\__\
//  \/__\:\/:/  /    /:/  \/__/    /:/  \/__/ \/__/~~/:/  /
//       \::/  /    /:/  /        /:/  /            /:/  /
//       /:/  /     \/__/         \/__/            /:/  /
//      /:/  /                                    /:/  /
//      \/__/                                     \/__/
//
// (c) Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use crate::library::enumerate_directory;
use crate::lookup::get_search_dirs;
use crate::{Config, PathData};

use fxhash::FxHashMap as HashMap;
use rayon::prelude::*;
use skim::prelude::*;
use std::path::PathBuf;
use std::{
    ffi::OsString,
    fs::DirEntry,
    io::{Stdout, Write},
    path::Path,
    sync::Arc,
    time::SystemTime,
};

pub fn deleted_exec(
    config: &Config,
    out: &mut Stdout,
) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
    // won't be sending anything anywhere, this just allows us to reuse enumerate_directory
    let (dummy_tx_item, _): (SkimItemSender, SkimItemReceiver) = unbounded();
    let config_clone = Arc::new(config.clone());

    enumerate_directory(
        config_clone,
        &dummy_tx_item,
        &config.requested_dir.path_buf,
        out,
    )?;

    // flush and exit successfully upon ending recursive search
    if config.opt_recursive {
        println!();
        out.flush()?;
    }
    std::process::exit(0)
}

pub fn get_deleted(
    config: &Config,
    path: &Path,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let immediate_dataset_deleted = vec![path]
        .into_iter().flat_map(|path| get_search_dirs(config, &PathData::from(path), false)).flat_map(|search_dirs| get_deleted_per_dataset(path, search_dirs))
        .flatten()
        .collect();

    let combined_deleted: Vec<PathData> = if config.opt_alt_replicated {
        let alt_replicated_deleted = vec![path]
            .into_iter().flat_map(|path| get_search_dirs(config, &PathData::from(path), true)).flat_map(|search_dirs| get_deleted_per_dataset(path, search_dirs))
            .flatten()
            .collect();

        [immediate_dataset_deleted, alt_replicated_deleted]
            .into_iter()
            .flatten()
            .collect()
    } else {
        immediate_dataset_deleted
    };

    // we need to make certain that what we return from possibly multiple datasets are unique
    // as these will be the filenames that populate our interactive views, so deduplicate
    // by system time and size here
    let unique_deleted = if config.opt_alt_replicated {
        let mut unique_deleted: HashMap<(&SystemTime, &u64), &PathData> = HashMap::default();

        combined_deleted.iter().for_each(|pathdata| {
            let _ = unique_deleted.insert((&pathdata.system_time, &pathdata.size), pathdata);
        });

        unique_deleted
            .into_iter()
            .map(|(_, v)| v)
            .cloned()
            .collect()
    } else {
        combined_deleted
    };

    Ok(unique_deleted)
}

fn get_deleted_per_dataset(
    path: &Path,
    search_dirs: (PathBuf, PathBuf),
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let (hidden_snapshot_dir, local_path) = search_dirs;

    // get all local entries we need to compare against these to know
    // what is a deleted file
    let local_dir_entries: Vec<DirEntry> = std::fs::read_dir(&path)?
        .into_iter()
        .par_bridge()
        .flatten()
        .collect();

    // create a collection of local unique file names
    let mut local_unique_filenames: HashMap<OsString, DirEntry> = HashMap::default();
    local_dir_entries.into_iter().for_each(|dir_entry| {
        let _ = local_unique_filenames.insert(dir_entry.file_name(), dir_entry);
    });

    // now create a collection of file names in the snap_dirs
    let snap_files: Vec<(OsString, DirEntry)> = std::fs::read_dir(&hidden_snapshot_dir)?
        .flatten()
        .par_bridge()
        .map(|entry| entry.path())
        .map(|path| path.join(&local_path))
        .map(|path| std::fs::read_dir(&path))
        .flatten_iter()
        .flatten_iter()
        .flatten_iter()
        .map(|dir_entry| (dir_entry.file_name(), dir_entry))
        .collect();

    // create a list of unique filenames on snaps
    let mut unique_snap_filenames: HashMap<OsString, DirEntry> = HashMap::default();
    snap_files.into_iter().for_each(|(file_name, dir_entry)| {
        let _ = unique_snap_filenames.insert(file_name, dir_entry);
    });

    // compare local filenames to all unique snap filenames - none values are unique here
    let deleted_pathdata = unique_snap_filenames
        .into_iter()
        .filter(|(file_name, _)| local_unique_filenames.get(file_name).is_none())
        .map(|(_, dir_entry)| PathData::from(&dir_entry));

    // deduplicate all by modify time and size - as we would elsewhere
    let mut unique_deleted_versions: HashMap<(SystemTime, u64), PathData> = HashMap::default();
    deleted_pathdata.for_each(|pathdata| {
        let _ = unique_deleted_versions.insert((pathdata.system_time, pathdata.size), pathdata);
    });

    let mut sorted: Vec<_> = unique_deleted_versions
        .into_iter()
        .map(|(_, v)| v)
        .collect();

    sorted.par_sort_unstable_by_key(|pathdata| pathdata.system_time);

    Ok(sorted)
}
