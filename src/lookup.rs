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

use crate::{Config, HttmError, PathData, SnapPoint};
use fxhash::FxHashMap as HashMap;
use rayon::prelude::*;
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

pub fn lookup_exec(
    config: &Config,
    path_data: &Vec<PathData>,
) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
    let all_snaps: Vec<PathData> = if config.opt_alt_replicated {
        // create vec of all local and replicated backups
        path_data
            .into_par_iter()
            .map(|path_data| {
                [
                    get_search_dirs(config, path_data, true),
                    get_search_dirs(config, path_data, false),
                ]
            })
            .flatten()
            .map(|search_dirs| search_dirs.ok())
            .flatten()
            .map(get_versions)
            .flatten()
            .flatten()
            .collect()
    } else {
        // create vec of most local dataset/user specified backups
        path_data
            .into_par_iter()
            .map(|path_data| get_search_dirs(config, path_data, false))
            .flatten()
            .map(get_versions)
            .flatten_iter()
            .flatten_iter()
            .collect::<Vec<PathData>>()
    };

    // create vec of live copies - unless user doesn't want it!
    let live_versions: Vec<PathData> = if !config.opt_no_live_vers {
        path_data.to_owned()
    } else {
        Vec::new()
    };

    // check if all files (snap and live) do not exist, if this is true, then user probably messed up
    // and entered a file that never existed (that is, perhaps a wrong file name)?
    if all_snaps.is_empty() && live_versions.iter().all(|i| i.is_phantom) {
        return Err(HttmError::new(
            "Neither a live copy, nor a snapshot copy of such a file appears to exist, so, umm, 🤷? Please try another file.",
        )
        .into());
    }

    Ok([all_snaps, live_versions])
}

pub fn get_search_dirs(
    config: &Config,
    file_pathdata: &PathData,
    for_alt_replicated: bool,
) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // which ZFS dataset do we want to use
    let file_path = &file_pathdata.path_buf;

    let (dataset, immediate_dataset_snap_mount) = match &config.snap_point {
        SnapPoint::UserDefined(defined_dirs) => (
            defined_dirs.snap_dir.to_owned(),
            defined_dirs.snap_dir.to_owned(),
        ),
        SnapPoint::Native(mount_collection) => {
            let immediate_dataset_snap_mount =
                get_immediate_dataset(file_pathdata, mount_collection)?;

            if for_alt_replicated {
                get_alt_replicated_dataset(&immediate_dataset_snap_mount, mount_collection)?
            } else {
                // ordinary case
                (
                    immediate_dataset_snap_mount.clone(),
                    immediate_dataset_snap_mount,
                )
            }
        }
    };

    // building the snapshot path from our dataset
    let hidden_snapshot_dir: PathBuf = [&dataset, &PathBuf::from(".zfs/snapshot")].iter().collect();

    // building our local relative path by removing parent
    // directories below the remote/snap mount point
    let local_path = match &config.snap_point {
        SnapPoint::UserDefined(defined_dirs) => {
            file_path
                .strip_prefix(&defined_dirs.local_dir).map_err(|_| HttmError::new("Are you sure you're in the correct working directory?  Perhaps you need to set the LOCAL_DIR value."))
        }
        SnapPoint::Native(_) => {
            // Note: this must be our most local snapshot mount to get get the correct relative path
            // this is why we need the original dataset even when we are searching an alternate filesystem
            // and cannot process all these items separately, in a multitude of functions
            file_path
                .strip_prefix(&immediate_dataset_snap_mount).map_err(|_| HttmError::new("Are you sure you're in the correct working directory?  Perhaps you need to set the SNAP_DIR and LOCAL_DIR values."))   
            }
    }?;

    Ok((hidden_snapshot_dir, local_path.to_path_buf()))
}

fn get_alt_replicated_dataset(
    immediate_dataset_snap_mount: &PathBuf,
    mount_collection: &[(String, String)],
) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let mut unique_mounts: HashMap<&Path, &String> = HashMap::default();

    // reverse the order - mount as key, fs as value
    mount_collection.iter().for_each(|(fs, mount)| {
        let _ = unique_mounts.insert(Path::new(mount), fs);
    });

    // so we can search for the mount as a key
    match &unique_mounts.get(&immediate_dataset_snap_mount.as_path()) {
        Some(immediate_dataset_fs_name) => {
            // find a filesystem that ends with our most local filesystem name
            // but has a preface name, like a different pool name: rpool might be
            // replicated to tank/rpool
            //
            // note: this won't work where dozer/rpool is replicated to tank/rpool, and
            // is only one way (one can only see the receiving fs from the sending fs),
            // but works well enough.  If you have a better idea, let me know.
            if let Some((alt_replicated_mount, _)) = unique_mounts
                .clone()
                .into_par_iter()
                .filter(|(_, fs)| fs.ends_with(immediate_dataset_fs_name.as_str()))
                .max_by_key(|(_, fs)| fs.len())
            {
                if &alt_replicated_mount.to_owned() == immediate_dataset_snap_mount {
                    return Err(HttmError::new("httm was unable to detect an alternate replicated mount point.  Perhaps the replicated filesystem is not mounted?").into());
                }
                Ok((
                    alt_replicated_mount.to_path_buf(),
                    immediate_dataset_snap_mount.clone(),
                ))
            } else {
                // could not find the some replicated mount
                Err(HttmError::new("httm was unable to detect an alternate replicated mount point.  Perhaps the replicated filesystem is not mounted?").into())
            }
        }
        None => {
            // could not find the immediate dataset
            Err(HttmError::new("httm was unable to detect an alternate replicated mount point.  Perhaps the replicated filesystem is not mounted?").into())
        }
    }
}

fn get_versions(
    search_dirs: (PathBuf, PathBuf),
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let (hidden_snapshot_dir, local_path) = search_dirs;

    // get the DirEntry for our snapshot path which will have all our possible
    // needed snapshots, like so: .zfs/snapshots/<some snap name>/, some snap name
    // are our entries here
    let versions = std::fs::read_dir(hidden_snapshot_dir)?
        .flatten()
        .par_bridge()
        .map(|entry| entry.path())
        .map(|path| path.join(&local_path))
        .map(|path| PathData::from(path.as_path()))
        .filter(|pathdata| !pathdata.is_phantom)
        .collect::<Vec<PathData>>();

    // filter above will remove all the phantom values silently as we build a list of unique versions
    // and our hashmap will then remove duplicates with the same system modify time and size/file len
    let mut unique_versions: HashMap<(SystemTime, u64), PathData> = HashMap::default();
    versions.into_iter().for_each(|pathdata| {
        let _ = unique_versions.insert((pathdata.system_time, pathdata.size), pathdata);
    });

    let mut sorted: Vec<PathData> = unique_versions.into_iter().map(|(_, v)| v).collect();

    sorted.par_sort_unstable_by_key(|pathdata| pathdata.system_time);

    Ok(sorted)
}

pub fn get_immediate_dataset(
    pathdata: &PathData,
    mount_collection: &Vec<(String, String)>,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let file_path = &pathdata.path_buf;

    // only possible None value is if root dir because
    // of previous work in the Pathdata new method, safe to unwrap or else here
    let parent_folder = file_path.parent().unwrap_or_else(|| Path::new("/"));

    // prune away most datasets by filtering - parent folder of file must contain relevant dataset
    let potential_mountpoints: Vec<&String> = mount_collection
        .into_par_iter()
        .map(|(_, mount)| mount)
        .filter(|line| parent_folder.starts_with(line))
        .collect();

    // do we have any left? if not print error
    if potential_mountpoints.is_empty() {
        let msg = "Could not identify any qualifying dataset.  Maybe consider specifying manually at SNAP_POINT?";
        return Err(HttmError::new(msg).into());
    };

    // select the best match for us: the longest, as we've already matched on the parent folder
    // so for /usr/bin, we would then prefer /usr/bin to /usr and /
    let best_potential_mountpoint =
        if let Some(some_bpmp) = potential_mountpoints.par_iter().max_by_key(|x| x.len()) {
            some_bpmp
        } else {
            let msg = format!(
                "There is no best match for a ZFS dataset to use for path {:?}. Sorry!/Not sorry?)",
                file_path
            );
            return Err(HttmError::new(&msg).into());
        };

    Ok(PathBuf::from(best_potential_mountpoint))
}
