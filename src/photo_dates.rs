//! Dates set by hand for photos and videos (Gallery → Set date): an exact date and time, or the
//! date the file has moved by an amount, for a camera whose clock was wrong.
//!
//! They are kept in OwnNAS's database, not written into the files: photos stay exactly as they
//! are (and as backed up, and as phones know them by their contents), every kind of photo and
//! video works the same, and undoing it is only forgetting the date. A date set by hand wins
//! over the camera's and the file name's, comes back when the photo index is rebuilt, and goes
//! wherever the file is moved or renamed (`db::rewrite_path_meta`).

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

/// The photo index's `date_source` for a date set by hand (a wall clock, kept as UTC).
pub const SOURCE: &str = "manual";

/// The most photos one change may touch.
pub const MAX_PATHS: usize = crate::albums::MAX_ITEMS_PER_CHANGE;

/// The earliest date that may be set (the first photographs are from the 1820s).
const EARLIEST: i64 = -4_733_510_400; // 1820-01-01

/// What to do with the photos' dates.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// This date and time (a wall clock, as seconds treated as UTC).
    At(i64),
    /// Later (or earlier, when negative) by this many seconds than they are now.
    Shift(i64),
    /// Back to their own dates: the camera's, the file name's, or the file's.
    Automatic,
    /// Each to the date given (`None`: its own), as an undo puts them back.
    Restore(Vec<Option<i64>>),
}

/// The date set by hand for `path`, if one is.
pub fn manual(conn: &Connection, path: &str) -> Option<i64> {
    conn.query_row("SELECT taken_at FROM photo_dates WHERE path = ?1", [path], |row| row.get(0)).optional().ok().flatten()
}

/// A Unix time (a real instant) as the wall clock it was here.
fn wall_clock_of(instant: i64) -> i64 {
    chrono::DateTime::from_timestamp(instant, 0).map_or(instant, |utc| utc.with_timezone(&chrono::Local).naive_local().and_utc().timestamp())
}

/// The date a photo shows under now, as a wall clock: the index's (a real instant for "utc" and
/// "file" dates, turned into this server's time), or read from the file when not indexed yet.
fn shown(conn: &Connection, root: &Path, path: &str) -> Option<i64> {
    let indexed: Option<(i64, String)> = conn
        .query_row("SELECT taken_at, date_source FROM media_index WHERE path = ?1", [path], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()
        .ok()
        .flatten();
    let (at, source) = match indexed {
        Some((at, source)) => (at, source),
        None => match manual(conn, path) {
            Some(at) => return Some(at),
            None => {
                let (at, source) = crate::media_index::automatic_date(&root.join(path), path)?;
                (at, source.to_string())
            }
        },
    };
    Some(if matches!(source.as_str(), "utc" | "file") { wall_clock_of(at) } else { at })
}

/// Whether a date set by hand is a plausible one: from 1820 up to tomorrow.
fn plausible(at: i64) -> bool {
    (EARLIEST..=wall_clock_of(crate::db::now_secs()) + 86_400).contains(&at)
}

/// What a change did: each photo's date set by hand before it (`None`: it had its own), so it
/// can be undone with `Change::Restore`.
#[derive(Debug, Default)]
pub struct Done {
    pub paths: Vec<String>,
    pub before: Vec<Option<i64>>,
}

/// Sets, shifts, or forgets the dates of `paths` (library paths of photos and videos, already
/// checked). Dates going back to automatic are read from the files first, so call this away from
/// the shared database lock when there are many: `db` is locked only to write.
pub fn change(db: &std::sync::Mutex<Connection>, root: &Path, paths: &[String], change: &Change, by: &str) -> Result<Done, String> {
    if paths.is_empty() {
        return Ok(Done::default());
    }
    if paths.len() > MAX_PATHS {
        return Err(format!("Choose up to {MAX_PATHS} photos at a time"));
    }
    if let Change::Restore(dates) = change {
        if dates.len() != paths.len() {
            return Err("Give one date for each photo".into());
        }
    }
    let lock = || db.lock().unwrap_or_else(|err| err.into_inner());
    // The new date of each, `None` to go back to its own.
    let wanted: Vec<Option<i64>> = {
        let conn = lock();
        paths
            .iter()
            .enumerate()
            .map(|(index, path)| match change {
                Change::At(at) => Ok(Some(*at)),
                Change::Shift(by) => shown(&conn, root, path)
                    .map(|at| Some(at.saturating_add(*by)))
                    .ok_or_else(|| format!("Could not read the date of “{}”", path.rsplit('/').next().unwrap_or(path))),
                Change::Automatic => Ok(None),
                Change::Restore(dates) => Ok(dates[index]),
            })
            .collect::<Result<_, String>>()?
    };
    if wanted.iter().flatten().any(|at| !plausible(*at)) {
        return Err("Choose a date from 1820 up to today".into());
    }
    // Their own dates, read from the files without holding the database.
    let own: Vec<Option<(i64, &'static str)>> =
        paths.iter().zip(&wanted).map(|(path, at)| if at.is_none() { crate::media_index::automatic_date(&root.join(path), path) } else { None }).collect();
    let now = crate::db::now_secs();
    let conn = lock();
    let tx = conn.unchecked_transaction().map_err(|_| "Could not change the dates".to_string())?;
    let sql = |_| "Could not change the dates".to_string();
    let mut done = Done::default();
    for ((path, at), own) in paths.iter().zip(&wanted).zip(own) {
        let before = manual(&tx, path);
        match at {
            Some(at) => {
                tx.execute(
                    "INSERT INTO photo_dates(path, taken_at, set_at, set_by) VALUES(?1, ?2, ?3, ?4)
                     ON CONFLICT(path) DO UPDATE SET taken_at = excluded.taken_at, set_at = excluded.set_at, set_by = excluded.set_by",
                    params![path, at, now, by],
                )
                .map_err(sql)?;
                tx.execute("UPDATE media_index SET taken_at = ?2, date_source = ?3 WHERE path = ?1", params![path, at, SOURCE]).map_err(sql)?;
            }
            None => {
                tx.execute("DELETE FROM photo_dates WHERE path = ?1", [path]).map_err(sql)?;
                match own {
                    Some((at, source)) => tx.execute("UPDATE media_index SET taken_at = ?2, date_source = ?3 WHERE path = ?1", params![path, at, source]),
                    // Not readable now: the next scan reads it again.
                    None => tx.execute("UPDATE media_index SET mtime = -1 WHERE path = ?1 AND date_source = ?2", params![path, SOURCE]),
                }
                .map_err(sql)?;
            }
        }
        done.paths.push(path.clone());
        done.before.push(before);
    }
    tx.commit().map_err(sql)?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn date(conn: &Connection, path: &str) -> (i64, String) {
        conn.query_row("SELECT taken_at, date_source FROM media_index WHERE path = ?1", [path], |row| Ok((row.get(0)?, row.get(1)?))).unwrap()
    }

    #[test]
    fn dates_set_by_hand_win_last_and_can_be_undone() {
        let scratch = Scratch(std::env::temp_dir().join(format!("ownnas-dates-{}", rand::random::<u64>())));
        let lib = scratch.0.join("library");
        std::fs::create_dir_all(lib.join("trip")).unwrap();
        std::fs::write(lib.join("trip/IMG-20210201-WA0003.jpg"), b"no exif here").unwrap();
        std::fs::write(lib.join("trip/clip.mp4"), b"not really a video").unwrap();
        let conn = crate::db::open(&scratch.0.join("data/ownnas.db")).unwrap();
        crate::media_index::scan(&conn, &lib, &|_, _| {}).unwrap();
        let (named, source) = date(&conn, "trip/IMG-20210201-WA0003.jpg");
        assert_eq!(source, "name");
        assert_eq!(named.div_euclid(86_400), 1_612_137_600 / 86_400, "1 February 2021, from the name");
        let db = std::sync::Mutex::new(conn);
        let photo = vec!["trip/IMG-20210201-WA0003.jpg".to_string()];

        // An exact date wins over the name's.
        let at = 1_600_000_000; // 2020-09-13 12:26:40
        let first = change(&db, &lib, &photo, &Change::At(at), "ada").unwrap();
        assert_eq!(first.before, [None]);
        assert_eq!(date(&db.lock().unwrap(), &photo[0]), (at, "manual".into()));

        // Shifting moves it from where it is now, and remembers the date it had.
        let second = change(&db, &lib, &photo, &Change::Shift(-86_400), "ada").unwrap();
        assert_eq!(second.before, [Some(at)]);
        assert_eq!(date(&db.lock().unwrap(), &photo[0]).0, at - 86_400);

        // It survives the photo changing and the index being rebuilt.
        std::fs::write(lib.join("trip/IMG-20210201-WA0003.jpg"), b"edited, still no exif").unwrap();
        {
            let conn = db.lock().unwrap();
            conn.execute("DELETE FROM media_index", []).unwrap();
            crate::media_index::scan(&conn, &lib, &|_, _| {}).unwrap();
            assert_eq!(date(&conn, &photo[0]), (at - 86_400, "manual".into()));
        }

        // It goes with the file when it is renamed, and is forgotten when it is deleted.
        {
            let conn = db.lock().unwrap();
            crate::db::rewrite_path_meta(&conn, "trip", "Italy").unwrap();
            assert_eq!(manual(&conn, "Italy/IMG-20210201-WA0003.jpg"), Some(at - 86_400));
            assert_eq!(manual(&conn, &photo[0]), None);
            crate::db::rewrite_path_meta(&conn, "Italy", "trip").unwrap();
        }

        // Undo puts back the date it had; going back to automatic gives the name's date again.
        change(&db, &lib, &photo, &Change::Restore(second.before.clone()), "ada").unwrap();
        assert_eq!(date(&db.lock().unwrap(), &photo[0]), (at, "manual".into()));
        let back = change(&db, &lib, &photo, &Change::Restore(first.before.clone()), "ada").unwrap();
        assert_eq!(back.before, [Some(at)]);
        assert_eq!(date(&db.lock().unwrap(), &photo[0]), (named, "name".into()));
        assert_eq!(manual(&db.lock().unwrap(), &photo[0]), None);

        // A video with only its file date shifts from that date as this server's clock shows it.
        let clip = vec!["trip/clip.mp4".to_string()];
        let (file_date, source) = date(&db.lock().unwrap(), &clip[0]);
        assert_eq!(source, "file");
        change(&db, &lib, &clip, &Change::Shift(7200), "ada").unwrap();
        assert_eq!(date(&db.lock().unwrap(), &clip[0]), (wall_clock_of(file_date) + 7200, "manual".into()));
        change(&db, &lib, &clip, &Change::Automatic, "ada").unwrap();
        assert_eq!(date(&db.lock().unwrap(), &clip[0]), (file_date, "file".into()));

        // Dates that cannot be right are refused, and nothing changes.
        assert!(change(&db, &lib, &photo, &Change::At(crate::db::now_secs() + 30 * 86_400), "ada").is_err());
        assert!(change(&db, &lib, &photo, &Change::Shift(-300 * 365 * 86_400), "ada").is_err());
        assert!(change(&db, &lib, &photo, &Change::Restore(vec![]), "ada").is_err());
        assert_eq!(manual(&db.lock().unwrap(), &photo[0]), None);

        // A copy keeps the date set by hand.
        change(&db, &lib, &photo, &Change::At(at), "ada").unwrap();
        {
            let conn = db.lock().unwrap();
            crate::db::copy_path_meta(&conn, &photo[0], "trip/copy.jpg").unwrap();
            assert_eq!(manual(&conn, "trip/copy.jpg"), Some(at));
            crate::db::delete_path_meta(&conn, "trip").unwrap();
            assert_eq!(manual(&conn, &photo[0]), None);
        }
    }
}
