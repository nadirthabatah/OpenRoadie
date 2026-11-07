//! A profile bundle: everything about a setup, in one folder you can copy.
//!
//! A profile file carries the configuration — bindings, per-app overlays,
//! camera settings. It does not carry a Stream Deck layout, because a layout
//! is its own file and may name icon files beside it. Someone moving to a new
//! computer wants *their setup*, not the half of it that fits in one file.
//!
//! A bundle is a directory, not an archive. That is deliberate: the promise
//! this project makes is that your settings are plain text you can read and
//! edit, and a zip would take that back for the sake of one fewer file to
//! copy. A folder can be inspected, diffed, and kept in git, and every tool a
//! person already has can move one.
//!
//! ```text
//! my-setup/
//!   config.toml        the configuration a profile file would hold
//!   layouts/
//!     streaming.toml   one per Stream Deck layout
//!     streaming/       any icons that layout names, if it has them
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The configuration file inside a bundle.
#[must_use]
pub fn config_in(bundle: &Path) -> PathBuf {
    bundle.join("config.toml")
}

/// The layouts directory inside a bundle.
#[must_use]
pub fn layouts_in(bundle: &Path) -> PathBuf {
    bundle.join("layouts")
}

/// Whether a path names a bundle rather than a single profile file.
///
/// A `.toml` path is a profile file; anything else is a bundle directory. The
/// rule is the same one the layout library uses for names versus paths, so
/// there is one thing to learn rather than two — and both commands say which
/// reading they took, so a wrong guess is corrected immediately rather than
/// discovered later.
#[must_use]
pub fn is_bundle(path: &Path) -> bool {
    !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
}

/// Copy every file in `from` into `to`, recursing into directories.
///
/// Used for a layout's icons, which sit in a directory beside it. Recursive
/// because a person may organise icons into folders, and a copy that silently
/// flattened or skipped them would produce a bundle that looks complete and
/// applies to a deck full of blank keys.
fn copy_tree(from: &Path, to: &Path, skipped: &mut Vec<PathBuf>) -> Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("failed to create {}", to.display()))?;
    let entries =
        std::fs::read_dir(from).with_context(|| format!("failed to read {}", from.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("failed to read {}", from.display()))?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        // `file_type` does not follow the link, which `Path::is_dir` does.
        // That distinction is the whole of this: a directory symlink pointing
        // anywhere at or above itself makes the recursion below unbounded, and
        // it recurses until the operating system refuses at around forty
        // levels, leaving forty levels of rubbish in a half-written bundle and
        // an error naming a path nobody can read.
        let kind = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", source.display()))?;
        if kind.is_symlink() && source.is_dir() {
            // Not followed, and not silent. A bundle is made to be carried to
            // another machine, where a link into this one's home directory
            // would dangle — so the person needs to know these did not travel,
            // rather than finding blank keys later.
            skipped.push(source);
        } else if kind.is_dir() {
            copy_tree(&source, &destination, skipped)?;
        } else {
            // A symlink to a *file* is followed, because `fs::copy` copies the
            // contents: an icon linked from a shared folder travels as an
            // icon, which is what someone who linked it wanted.
            std::fs::copy(&source, &destination)
                .with_context(|| format!("failed to copy {}", source.display()))?;
        }
    }
    Ok(())
}

/// What gathering the layout library into a bundle produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Gathered {
    /// Layouts carried from the library.
    pub carried: Vec<String>,
    /// Layouts already in the bundle that the library no longer has.
    ///
    /// Exporting twice to the same folder copies over what is there without
    /// removing anything, because that folder is a path the person named and
    /// deleting inside it on their behalf is not a risk worth taking for
    /// tidiness. But a layout they deleted months ago still riding along in
    /// what they think is a current backup is a surprise, so it is named.
    pub left_over: Vec<String>,
    /// Directory symlinks that were not followed, and so did not travel.
    pub skipped_links: Vec<PathBuf>,
}

/// Copy the layout library into a bundle.
///
/// A missing library is not a failure: a machine with no saved layouts has a
/// setup that is entirely configuration, and that is a complete bundle rather
/// than a broken one.
pub fn gather_layouts(library: &Path, bundle: &Path) -> Result<Gathered> {
    let destination = layouts_in(bundle);
    let before = names_in(&destination);
    if !library.is_dir() {
        // "It is not there" and "it is there and is not a folder" produce the
        // same empty result and mean opposite things. A machine that has saved
        // no layouts has a complete setup; a library that is a file is broken,
        // and reporting a successful export over it hands someone a bundle
        // they will discover is short on the machine they carried it to.
        if library.exists() {
            return Err(anyhow::anyhow!(
                "{} exists but is not a folder, so no layout could be saved there \
                 and none can be carried. Move or delete that file.",
                library.display()
            ));
        }
        return Ok(Gathered {
            carried: Vec::new(),
            left_over: before,
            skipped_links: Vec::new(),
        });
    }
    let mut skipped_links = Vec::new();
    copy_tree(library, &destination, &mut skipped_links)?;
    let carried = names_in(library);
    let left_over = before
        .into_iter()
        .filter(|name| !carried.contains(name))
        .collect();
    Ok(Gathered {
        carried,
        left_over,
        skipped_links,
    })
}

/// Copy a bundle's layouts back into the library.
///
/// Existing layouts of the same name are overwritten, which is what importing
/// a setup means. Layouts the library has and the bundle does not are left
/// alone: an import adds a setup rather than replacing the machine, and
/// deleting work someone did on this machine because it was not in a bundle
/// made elsewhere would be a far worse failure than an extra layout.
///
/// The configuration is backed up before an import for the reason it always
/// was; layouts are not, because a layout is a file the person chose to keep
/// and can see, not hidden state.
pub fn restore_layouts(bundle: &Path, library: &Path) -> Result<Vec<String>> {
    let source = layouts_in(bundle);
    if !source.is_dir() {
        return Ok(Vec::new());
    }
    // A bundle made by this program has no directory symlinks in it, so
    // anything skipped here came from a bundle someone assembled by hand;
    // dropped rather than followed, for the same reason as on the way out.
    let mut skipped = Vec::new();
    copy_tree(&source, library, &mut skipped)?;
    Ok(names_in(&source))
}

/// The layout names in a directory, sorted.
fn names_in(directory: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()? != "toml" {
                return None;
            }
            Some(path.file_stem()?.to_string_lossy().into_owned())
        })
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{config_in, gather_layouts, is_bundle, layouts_in, restore_layouts};

    /// A scratch directory that cleans itself up.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "roadie-bundle-{tag}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |since| since.as_nanos())
            ));
            std::fs::create_dir_all(&path).expect("a scratch directory");
            Self(path)
        }

        fn join(&self, tail: &str) -> PathBuf {
            self.0.join(tail)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("a parent directory");
        }
        std::fs::write(path, body).expect("a file");
    }

    #[test]
    fn a_toml_path_is_a_profile_file_and_anything_else_is_a_bundle() {
        assert!(!is_bundle(Path::new("setup.toml")));
        assert!(!is_bundle(Path::new("Setup.TOML")));
        assert!(is_bundle(Path::new("my-setup")));
        assert!(is_bundle(Path::new("/backups/desk")));
    }

    #[test]
    fn a_bundle_names_its_parts_predictably() {
        let bundle = Path::new("/x/setup");
        assert_eq!(config_in(bundle), PathBuf::from("/x/setup/config.toml"));
        assert_eq!(layouts_in(bundle), PathBuf::from("/x/setup/layouts"));
    }

    /// A machine with no saved layouts has a setup that is entirely
    /// configuration. That is a complete bundle, not a broken one.
    #[test]
    fn a_library_that_does_not_exist_yet_makes_an_empty_layout_set() {
        let scratch = Scratch::new("empty");
        let gathered = gather_layouts(&scratch.join("nothing-here"), &scratch.join("bundle"))
            .expect("an absent library is not a failure");
        assert_eq!(gathered.carried.len(), 0);
        assert_eq!(gathered.left_over.len(), 0);
    }

    /// "It is not there" and "it is there and is not a folder" produce the
    /// same empty result and mean opposite things. Reporting a successful
    /// export over the second hands someone a bundle they discover is short
    /// on the machine they carried it to — which is the one place they cannot
    /// go back and fix it from.
    #[test]
    fn a_library_that_is_a_file_is_an_error_rather_than_an_empty_export() {
        let scratch = Scratch::new("library-is-a-file");
        let library = scratch.join("library");
        write(&library, "this is a file, not a folder");

        let error = gather_layouts(&library, &scratch.join("bundle"))
            .expect_err("a broken library must not read as an empty one")
            .to_string();
        assert!(error.contains("not a folder"), "{error}");
        assert!(
            error.contains(&library.display().to_string()),
            "the message must name the path to fix: {error}"
        );
    }

    #[test]
    fn gathering_carries_every_layout_by_name() {
        let scratch = Scratch::new("gather");
        let library = scratch.join("library");
        write(&library.join("streaming.toml"), "brightness = 80\n");
        write(&library.join("work.toml"), "brightness = 50\n");
        let bundle = scratch.join("bundle");

        let gathered = gather_layouts(&library, &bundle).expect("gathered");
        assert_eq!(
            gathered.carried,
            vec!["streaming".to_owned(), "work".to_owned()]
        );
        assert!(layouts_in(&bundle).join("streaming.toml").is_file());
    }

    /// A directory symlink pointing at its own parent must not be followed.
    ///
    /// Following it recurses until the operating system refuses at around
    /// forty levels, and what it leaves behind is forty levels of duplicated
    /// rubbish in a half-written bundle plus an error naming a path nobody can
    /// read. Someone's layouts folder can contain a link like this for
    /// perfectly ordinary reasons — a shared icon set, a tidy-up that went
    /// sideways — and it is not a state they would think to check for before
    /// exporting.
    ///
    /// The guard was here and nothing tested it, which a mutation sweep found
    /// by removing it and watching every test still pass.
    ///
    /// Unix only. Windows has directory symlinks, but creating one needs
    /// either developer mode or an elevated process, so a test that made one
    /// would fail on an ordinary machine for a reason that has nothing to do
    /// with the code under test. The guard itself is not
    /// platform-conditional; only the making of the link to try it against is.
    #[cfg(unix)]
    #[test]
    fn a_directory_symlink_pointing_at_its_own_parent_is_skipped_not_followed() {
        let scratch = Scratch::new("symlink-loop");
        let library = scratch.join("library");
        write(&library.join("streaming.toml"), "brightness = 80\n");
        // A link back to the folder that contains it: the shape that recurses.
        std::os::unix::fs::symlink(&library, library.join("loop")).expect("a directory symlink");

        let bundle = scratch.join("bundle");
        let gathered = gather_layouts(&library, &bundle).expect("gathering must terminate");

        assert_eq!(
            gathered.carried,
            vec!["streaming".to_owned()],
            "the real layout must still travel"
        );
        assert_eq!(
            gathered.skipped_links.len(),
            1,
            "the link must be reported, not silently dropped: {:?}",
            gathered.skipped_links
        );

        // And nothing recursed: the deepest path in the bundle is the one
        // layout file, not forty copies of the folder inside itself.
        let deepest = walk_depth(&bundle);
        assert!(
            deepest <= 2,
            "the bundle nests {deepest} levels deep, so the link was followed"
        );
    }

    /// How many directory levels deep a tree goes.
    #[cfg(unix)]
    fn walk_depth(root: &Path) -> usize {
        let Ok(entries) = std::fs::read_dir(root) else {
            return 0;
        };
        entries
            .filter_map(Result::ok)
            .map(|entry| {
                if entry.path().is_dir() {
                    1 + walk_depth(&entry.path())
                } else {
                    1
                }
            })
            .max()
            .unwrap_or(0)
    }

    /// The failure this exists to prevent: a bundle that looks complete and
    /// applies to a deck full of blank keys, because the icons a layout names
    /// were left behind.
    #[test]
    fn a_layouts_icons_travel_with_it() {
        let scratch = Scratch::new("icons");
        let library = scratch.join("library");
        write(&library.join("streaming.toml"), "brightness = 80\n");
        write(&library.join("streaming/camera.png"), "not really a png");
        write(&library.join("streaming/deep/mic.png"), "nor this");
        let bundle = scratch.join("bundle");

        gather_layouts(&library, &bundle).expect("gathered");
        let carried = layouts_in(&bundle);
        assert!(carried.join("streaming/camera.png").is_file());
        assert!(
            carried.join("streaming/deep/mic.png").is_file(),
            "a nested icon directory must not be flattened or skipped"
        );
    }

    /// An icon file must not be offered as a layout you could apply.
    #[test]
    fn only_layout_files_are_counted_as_layouts() {
        let scratch = Scratch::new("counting");
        let library = scratch.join("library");
        write(&library.join("streaming.toml"), "brightness = 80\n");
        write(&library.join("streaming/camera.png"), "an icon");
        let bundle = scratch.join("bundle");

        let gathered = gather_layouts(&library, &bundle).expect("gathered");
        assert_eq!(gathered.carried, vec!["streaming".to_owned()]);
    }

    /// Exporting twice to the same folder does not remove what is there —
    /// that folder is a path the person named, and deleting inside it on
    /// their behalf is not a risk worth taking for tidiness. But a layout
    /// deleted months ago still riding along in what looks like a current
    /// backup is a surprise, so it has to be named.
    #[test]
    fn a_layout_left_behind_by_an_earlier_export_is_reported() {
        let scratch = Scratch::new("stale");
        let library = scratch.join("library");
        write(&library.join("streaming.toml"), "brightness = 80\n");
        write(&library.join("work.toml"), "brightness = 50\n");
        let bundle = scratch.join("bundle");
        gather_layouts(&library, &bundle).expect("first export");

        std::fs::remove_file(library.join("work.toml")).expect("delete a layout");
        let gathered = gather_layouts(&library, &bundle).expect("second export");

        assert_eq!(gathered.carried, vec!["streaming".to_owned()]);
        assert_eq!(
            gathered.left_over,
            vec!["work".to_owned()],
            "the deleted layout is still in the folder and must be named"
        );
    }

    /// The whole point: a bundle made on one machine puts the layouts back on
    /// another, icons included.
    #[test]
    fn a_bundle_restores_onto_a_machine_with_nothing_on_it() {
        let scratch = Scratch::new("restore");
        let library = scratch.join("library");
        write(&library.join("streaming.toml"), "brightness = 80\n");
        write(&library.join("streaming/camera.png"), "an icon");
        let bundle = scratch.join("bundle");
        gather_layouts(&library, &bundle).expect("gathered");

        let fresh = scratch.join("fresh-machine");
        let restored = restore_layouts(&bundle, &fresh).expect("restored");
        assert_eq!(restored, vec!["streaming".to_owned()]);
        assert!(fresh.join("streaming.toml").is_file());
        assert!(fresh.join("streaming/camera.png").is_file());
    }

    /// An import adds a setup; it does not replace the machine. Deleting work
    /// someone did here because it was not in a bundle made elsewhere would be
    /// a far worse failure than an extra layout.
    #[test]
    fn importing_does_not_delete_a_layout_this_machine_already_had() {
        let scratch = Scratch::new("additive");
        let bundle = scratch.join("bundle");
        write(
            &layouts_in(&bundle).join("streaming.toml"),
            "brightness = 80\n",
        );
        let library = scratch.join("library");
        write(&library.join("mine.toml"), "brightness = 10\n");

        restore_layouts(&bundle, &library).expect("restored");
        assert!(
            library.join("mine.toml").is_file(),
            "a layout made on this machine must survive an import"
        );
        assert!(library.join("streaming.toml").is_file());
    }

    /// A profile file exported the old way carries no layouts. Restoring from
    /// it must be an ordinary empty result, not a failure — that is still a
    /// valid thing to hand this function.
    #[test]
    fn a_bundle_with_no_layouts_restores_nothing_without_complaining() {
        let scratch = Scratch::new("layoutless");
        let bundle = scratch.join("bundle");
        write(&config_in(&bundle), "schema_version = 1\n");
        let restored = restore_layouts(&bundle, &scratch.join("library")).expect("not a failure");
        assert_eq!(restored.len(), 0);
    }
}
