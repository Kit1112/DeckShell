//! Clips that Steam wrote to disk: `userdata/<acct>/gamerecordings/clips/<clip_id>/` with `clip.pb` (summary), `thumbnail.jpg`,
//! `timelines/*.json` and `video/<recording>/session.mpd` (+ m4s chunks). A clip may span several recording folders.
//! Steam's in-memory clip cache stays empty until something asks for it, so Media lists these folders directly. Read-only:
//! nothing here writes into Steam's folders; the only file written is the concat list for multi-recording playback.
//!
//! Field meanings in `clip.pb` were checked against Steam's own clip summary on the Deck (clip 11640853602745974784_20261109_042652:
//! entry duration 21816 ms, size 15843111 bytes, the same values as the native summary in the contract). Field 2 is the start
//! offset (386906 matches the native `start_offset_ms`); the other fields are used only where noted.
use std::path::{Path, PathBuf};

use steam_bridge::recording::{parse_clip_summary, ClipSummary};

/// Parsed folder name `clip_<gameid>_<YYYYMMDD>_<HHMMSS>` (UTC, as Steam writes it). Returns (game id, Unix seconds).
pub(crate) fn parse_clip_dir_name(name: &str) -> Option<(String, u32)> {
    let mut parts = name.strip_prefix("clip_")?.split('_');
    let (gid, date, time) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() { return None; }
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(gid) || gid.starts_with('0') || gid.parse::<u64>().is_err() { return None; }
    if date.len() != 8 || time.len() != 6 || !digits(date) || !digits(time) { return None; }
    let num = |s: &str| s.parse::<i64>().ok();
    let (y, mo, d) = (num(&date[0..4])?, num(&date[4..6])?, num(&date[6..8])?);
    let (h, mi, s) = (num(&time[0..2])?, num(&time[2..4])?, num(&time[4..6])?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 { return None; }
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3_600 + mi * 60 + s;
    Some((gid.to_string(), u32::try_from(secs).ok()?))
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn varint(b: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let mut v = 0u64;
    let mut shift = 0u32;
    loop {
        let c = *b.get(i)?;
        i += 1;
        if shift >= 64 { return None; }
        v |= u64::from(c & 0x7f) << shift;
        if c < 0x80 { return Some((v, i)); }
        shift += 7;
    }
}

/// Fields read from `clip.pb`: `duration_ms` is the sum of the timeline entries' durations (entry field 4).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ClipPb { pub duration_ms: u64, pub file_size: Option<u64>, pub date_clipped: Option<u64> }

/// Decodes the protobuf wire format of `clip.pb`. None when the bytes are not well-formed.
pub(crate) fn read_clip_pb(b: &[u8]) -> Option<ClipPb> {
    let mut out = ClipPb::default();
    let mut i = 0;
    while i < b.len() {
        let (key, n) = varint(b, i)?;
        i = n;
        let field = key >> 3;
        match key & 7 {
            0 => {
                let (v, n) = varint(b, i)?;
                i = n;
                match field {
                    3 => out.date_clipped = Some(v),
                    6 => out.file_size = Some(v),
                    _ => {}
                }
            }
            2 => {
                let (len, n) = varint(b, i)?;
                let end = n.checked_add(usize::try_from(len).ok()?)?;
                let body = b.get(n..end)?;
                i = end;
                if field == 1 { out.duration_ms = out.duration_ms.checked_add(entry_duration(body)?)?; }
            }
            1 => i = i.checked_add(8)?,
            5 => i = i.checked_add(4)?,
            _ => return None,
        }
    }
    Some(out)
}

fn entry_duration(b: &[u8]) -> Option<u64> {
    let mut i = 0;
    let mut duration = 0;
    while i < b.len() {
        let (key, n) = varint(b, i)?;
        i = n;
        match key & 7 {
            0 => {
                let (v, n) = varint(b, i)?;
                i = n;
                if key >> 3 == 4 { duration = v; }
            }
            2 => {
                let (len, n) = varint(b, i)?;
                i = n.checked_add(usize::try_from(len).ok()?)?;
            }
            1 => i = i.checked_add(8)?,
            5 => i = i.checked_add(4)?,
            _ => return None,
        }
    }
    Some(duration)
}

/// Duration of a DASH MPD from its `mediaPresentationDuration` (ISO 8601 `PT#H#M#S`), in milliseconds.
pub(crate) fn mpd_duration_ms(text: &str) -> Option<u64> {
    const KEY: &str = "mediaPresentationDuration=\"P";
    let start = text.find(KEY)? + KEY.len();
    let body = &text[start..];
    let body = &body[..body.find('"')?];
    let (mut total, mut num, mut in_time) = (0f64, String::new(), false);
    for c in body.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' | '.' => num.push(c),
            unit => {
                let v: f64 = num.parse().ok()?;
                num.clear();
                total += match (unit, in_time) {
                    ('D', false) => v * 86_400.0,
                    ('H', true) => v * 3_600.0,
                    ('M', true) => v * 60.0,
                    ('S', true) => v,
                    _ => return None,
                };
            }
        }
    }
    if !num.is_empty() || !total.is_finite() { return None; }
    Some((total * 1000.0).round() as u64)
}

/// Recording folders of a clip that have a manifest, in recording order (names embed the start time).
pub(crate) fn recording_manifests(clip_dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(clip_dir.join("video")) else { return vec![] };
    let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && p.join("session.mpd").is_file()).collect();
    dirs.sort();
    dirs.into_iter().map(|d| d.join("session.mpd")).collect()
}

/// What playback needs: the manifests in order and the media duration (sum of the manifests).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PlayPlan { pub manifests: Vec<PathBuf>, pub duration_ms: u64 }

/// Reads only; None when the clip has no playable recording.
pub(crate) fn play_plan(clip_dir: &Path) -> Option<PlayPlan> {
    let manifests = recording_manifests(clip_dir);
    if manifests.is_empty() { return None; }
    let mut duration_ms = 0u64;
    for m in &manifests {
        let text = std::fs::read_to_string(m).ok()?;
        duration_ms = duration_ms.checked_add(mpd_duration_ms(&text)?)?;
    }
    Some(PlayPlan { manifests, duration_ms })
}

/// Folder size of the clip, two levels deep (clip.pb missing or unreadable: the list still shows the clip).
fn dir_bytes(dir: &Path, depth: u8) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten().map(|e| match e.metadata() {
        Ok(m) if m.is_file() => m.len(),
        Ok(m) if m.is_dir() && depth > 0 => dir_bytes(&e.path(), depth - 1),
        _ => 0,
    }).sum()
}

/// Clip summaries of one account's clip folders. Folders whose name does not follow Steam's pattern are skipped.
pub(crate) fn disk_clip_summaries(steam_root: &Path, acct: u32) -> Vec<ClipSummary> {
    let clips = steam_root.join("userdata").join(acct.to_string()).join("gamerecordings").join("clips");
    let Ok(rd) = std::fs::read_dir(&clips) else { return vec![] };
    let mut out = vec![];
    for e in rd.flatten() {
        let dir = e.path();
        let Some(name) = e.file_name().to_str().map(str::to_owned) else { continue };
        if !dir.is_dir() { continue; }
        let Some((game_id, recorded)) = parse_clip_dir_name(&name) else { continue };
        let pb = std::fs::read(dir.join("clip.pb")).ok().and_then(|b| read_clip_pb(&b));
        let (duration_ms, file_size, date_clipped) = match &pb {
            Some(pb) => (pb.duration_ms, pb.file_size.unwrap_or_else(|| dir_bytes(&dir, 2)), pb.date_clipped.unwrap_or(0)),
            None => {
                let ms = play_plan(&dir).map(|p| p.duration_ms).unwrap_or(0);
                (ms, dir_bytes(&dir, 2), 0)
            }
        };
        let thumb = if dir.join("thumbnail.jpg").is_file() {
            format!("https://steamloopback.host/gamerecordings/clips/{name}/thumbnail.jpg")
        } else {
            String::new()
        };
        let value = serde_json::json!({
            "clip_id": name,
            "game_id": game_id,
            "duration_ms": duration_ms.to_string(),
            "date_recorded": recorded,
            "file_size": file_size.to_string(),
            "date_clipped": u32::try_from(date_clipped).unwrap_or(0),
            "temporary": false,
            "thumbnail_url": thumb,
        });
        if let Ok(c) = parse_clip_summary(&value) { out.push(c); }
    }
    out
}

/// The folder of `clip_id` under `clips_root`, only when `clip_id` is exactly one folder name that follows the clip
/// pattern and the joined path sits directly inside `clips_root`. Every path used to delete a clip goes through here.
pub(crate) fn clip_dir_for_delete(clips_root: &Path, clip_id: &str) -> Option<PathBuf> {
    parse_clip_dir_name(clip_id)?;
    let mut parts = Path::new(clip_id).components();
    match (parts.next(), parts.next()) {
        (Some(std::path::Component::Normal(_)), None) => {}
        _ => return None,
    }
    let dir = clips_root.join(clip_id);
    (dir.parent() == Some(clips_root)).then_some(dir)
}

/// Input of the ffmpeg process: one manifest, or a concat list for several recordings (written once per clip).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Input { pub path: PathBuf, pub concat: bool }

/// Single-recording clips play their manifest directly; several recordings go through a concat list in `cache_dir`.
pub(crate) fn materialize(plan: &PlayPlan, clip_id: &str, cache_dir: &Path) -> std::io::Result<Input> {
    if let [only] = plan.manifests.as_slice() {
        return Ok(Input { path: only.clone(), concat: false });
    }
    let dir = cache_dir.join("clip-play");
    std::fs::create_dir_all(&dir)?;
    let list = dir.join(format!("{clip_id}.txt"));
    let mut text = String::new();
    for m in &plan.manifests {
        let quoted = m.to_string_lossy().replace('\'', "'\\''");
        text.push_str(&format!("file '{quoted}'\n"));
    }
    std::fs::write(&list, text)?;
    Ok(Input { path: list, concat: true })
}

/// ffmpeg arguments: decodes video as RGBA frames at `fps` to stdout and sends the audio to PulseAudio/PipeWire.
/// `-re` keeps both outputs in real time, so the frame reader paces the process.
pub(crate) fn ffmpeg_args(input: &Input, start_ms: u64, w: u32, h: u32, fps: u32, audio: bool) -> Vec<String> {
    let mut a: Vec<String> = ["-nostdin", "-hide_banner", "-loglevel", "error", "-re", "-threads", "2"].map(String::from).into();
    if input.concat { a.extend(["-f", "concat", "-safe", "0"].map(String::from)); }
    a.extend(["-ss".into(), format!("{:.3}", start_ms as f64 / 1000.0), "-i".into(), input.path.to_string_lossy().into_owned()]);
    a.extend(["-map", "0:v:0", "-vf", &format!("fps={fps},scale={w}:{h}"), "-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1"].map(String::from));
    if audio {
        a.extend(["-map", "0:a:0?", "-f", "pulse", "DeckShell clip"].map(String::from));
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_path_is_only_a_direct_child_named_like_a_clip() {
        let root = Path::new("/steam/userdata/42/gamerecordings/clips");
        let id = "clip_2379780_20261112_040949";
        assert_eq!(clip_dir_for_delete(root, id), Some(root.join(id)));
        for bad in ["", ".", "..", "../clip_2379780_20261112_040949", "clip_2379780_20261112_040949/../other",
            "clip_2379780_20261112_040949/", "/etc/clip_2379780_20261112_040949", "notes", "clip_2379780_20261112_04094"] {
            assert_eq!(clip_dir_for_delete(root, bad), None, "{bad:?} must not resolve to a delete target");
        }
    }

    /// Protobuf encoder for test fixtures only.
    fn enc_varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 { out.push(b); return; }
            out.push(b | 0x80);
        }
    }
    fn field_varint(f: u64, v: u64, out: &mut Vec<u8>) { enc_varint(f << 3, out); enc_varint(v, out); }
    fn field_msg(f: u64, body: &[u8], out: &mut Vec<u8>) { enc_varint((f << 3) | 2, out); enc_varint(body.len() as u64, out); out.extend_from_slice(body); }

    fn sample_clip_pb(duration: u64, size: u64) -> Vec<u8> {
        let mut entry = vec![];
        field_varint(2, 1234, &mut entry);
        field_varint(4, duration, &mut entry);
        field_msg(5, &[0x0a, 0x02, b'f', b'g'], &mut entry); // nested recording message is skipped
        let mut out = vec![];
        field_msg(1, &entry, &mut out);
        field_varint(2, 630_639, &mut out);
        field_varint(3, 1_794_456_570, &mut out);
        field_varint(6, size, &mut out);
        field_varint(12, 1280, &mut out);
        out
    }

    #[test]
    fn folder_name_gives_game_id_and_utc_time() {
        let (gid, t) = parse_clip_dir_name("clip_11640853602745974784_20261109_042652").unwrap();
        assert_eq!(gid, "11640853602745974784", "64-bit shortcut GameID is kept as text");
        assert_eq!(t, 1_794_198_412, "2026-11-09 04:26:52 UTC");
        assert_eq!(parse_clip_dir_name("clip_2379780_20261112_040949").unwrap().0, "2379780");
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }

    #[test]
    fn folder_names_that_do_not_follow_the_pattern_are_refused() {
        for bad in ["clip_", "clip_0_20261112_040949", "clip_12_2026111_040949", "clip_12_20261312_040949", "clip_12_20261112_250000",
                    "clip_12_20261112_040949_x", "clip_ab_20261112_040949", "fg_12_20261112_040949", "clip_99999999999999999999999_20261112_040949"] {
            assert!(parse_clip_dir_name(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn clip_pb_gives_entry_duration_size_and_date() {
        let pb = read_clip_pb(&sample_clip_pb(21_816, 15_843_111)).unwrap();
        assert_eq!(pb, ClipPb { duration_ms: 21_816, file_size: Some(15_843_111), date_clipped: Some(1_794_456_570) });
        let two = [sample_clip_pb(10_000, 1), sample_clip_pb(5_000, 1)].concat();
        assert_eq!(read_clip_pb(&two).unwrap().duration_ms, 15_000, "timeline entries add up");
        assert!(read_clip_pb(&[0x0a, 0x09, 0x01]).is_none(), "truncated message is rejected");
        assert!(read_clip_pb(&[0x0b]).is_none(), "group wire type is rejected");
    }

    #[test]
    fn manifest_duration_reads_iso_durations() {
        let mpd = |d: &str| format!("<MPD mediaPresentationDuration=\"{d}\" minBufferTime=\"PT6.0S\">");
        assert_eq!(mpd_duration_ms(&mpd("PT18.25S")), Some(18_250));
        assert_eq!(mpd_duration_ms(&mpd("PT1M2.5S")), Some(62_500));
        assert_eq!(mpd_duration_ms(&mpd("PT1H")), Some(3_600_000));
        assert_eq!(mpd_duration_ms(&mpd("P1DT0S")), Some(86_400_000));
        assert_eq!(mpd_duration_ms(&mpd("PT5X")), None);
        assert_eq!(mpd_duration_ms("<MPD>"), None);
    }

    /// Makes a clip folder with the given recording folders (each with a manifest) and a clip.pb.
    fn clip_fixture(tag: &str, recordings: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("deckshell-media-clips-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let clip = root.join("clip_2379780_20261112_040949");
        for (dir, dur) in recordings {
            let rec = clip.join("video").join(dir);
            std::fs::create_dir_all(&rec).unwrap();
            std::fs::write(rec.join("session.mpd"), format!("<MPD mediaPresentationDuration=\"PT{dur}S\">")).unwrap();
            std::fs::write(rec.join("chunk-stream0-00001.m4s"), vec![0u8; 100]).unwrap();
        }
        std::fs::write(clip.join("clip.pb"), sample_clip_pb(18_670, 4_000)).unwrap();
        clip
    }

    #[test]
    fn recordings_are_played_in_order_and_summed() {
        let clip = clip_fixture("order", &[("fg_2379780_20261112_132731", "10.488"), ("fg_2379780_20261112_132705", "25.698")]);
        let plan = play_plan(&clip).unwrap();
        assert_eq!(plan.manifests.len(), 2);
        assert!(plan.manifests[0].to_string_lossy().contains("132705"), "earlier recording first: {:?}", plan.manifests);
        assert_eq!(plan.duration_ms, 36_186);
        let cache = std::env::temp_dir().join(format!("deckshell-media-cache-{}-order", std::process::id()));
        let input = materialize(&plan, "clip_2379780_20261112_040949", &cache).unwrap();
        assert!(input.concat);
        let list = std::fs::read_to_string(&input.path).unwrap();
        assert_eq!(list.lines().count(), 2);
        assert!(list.starts_with("file '") && list.lines().all(|l| l.starts_with("file '") && l.ends_with("session.mpd'")));
    }

    #[test]
    fn single_recording_plays_its_manifest_without_a_list() {
        let clip = clip_fixture("single", &[("fg_2379780_20261112_225439", "18.25")]);
        let plan = play_plan(&clip).unwrap();
        let input = materialize(&plan, "clip_2379780_20261112_040949", Path::new("/unused")).unwrap();
        assert_eq!(input, Input { path: plan.manifests[0].clone(), concat: false });
        assert_eq!(plan.duration_ms, 18_250);
    }

    #[test]
    fn disk_clips_become_native_summaries_for_the_media_grid() {
        let root = std::env::temp_dir().join(format!("deckshell-media-disk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let clip = root.join("userdata/42/gamerecordings/clips/clip_2379780_20261112_040949");
        std::fs::create_dir_all(&clip).unwrap();
        std::fs::write(clip.join("clip.pb"), sample_clip_pb(18_670, 4_000)).unwrap();
        std::fs::write(clip.join("thumbnail.jpg"), b"x").unwrap();
        // A folder that is not a clip, and a clip without clip.pb (duration from the manifest, size from the files).
        std::fs::create_dir_all(root.join("userdata/42/gamerecordings/clips/notes")).unwrap();
        let broken = root.join("userdata/42/gamerecordings/clips/clip_239030_20261113_132742");
        std::fs::create_dir_all(broken.join("video/fg_239030_20261113_132731")).unwrap();
        let mpd = "<MPD mediaPresentationDuration=\"PT10.5S\">";
        std::fs::write(broken.join("video/fg_239030_20261113_132731/session.mpd"), mpd).unwrap();
        std::fs::write(broken.join("video/fg_239030_20261113_132731/init.m4s"), vec![0u8; 50]).unwrap();

        let clips = disk_clip_summaries(&root, 42);
        assert_eq!(clips.len(), 2, "the folder without the clip pattern is skipped");
        let by_id = |id: &str| clips.iter().find(|c| c.clip_id == id).expect("listed");
        let b = by_id("clip_239030_20261113_132742");
        // Without clip.pb: duration from the manifest, size = every file in the clip folder.
        assert_eq!((b.duration_ms, b.file_size, b.thumbnail_url.as_str()), (10_500, mpd.len() as u64 + 50, ""));
        let a = by_id("clip_2379780_20261112_040949");
        assert_eq!(a.duration_ms, 18_670);
        assert_eq!(a.file_size, 4_000);
        assert_eq!(a.game_id.as_str(), "2379780");
        assert_eq!(a.thumbnail_url, "https://steamloopback.host/gamerecordings/clips/clip_2379780_20261112_040949/thumbnail.jpg");
        assert!(!a.temporary);
    }

    #[test]
    fn ffmpeg_arguments_keep_video_and_audio_in_one_process() {
        let input = Input { path: PathBuf::from("/x/session.mpd"), concat: false };
        let args = ffmpeg_args(&input, 12_345, 640, 400, 30, true);
        let joined = args.join(" ");
        assert!(joined.contains("-re") && joined.contains("-ss 12.345 -i /x/session.mpd"), "{joined}");
        assert!(joined.contains("fps=30,scale=640:400 -pix_fmt rgba -f rawvideo pipe:1"), "{joined}");
        assert!(joined.contains("-map 0:a:0? -f pulse"), "{joined}");
        assert!(!ffmpeg_args(&input, 0, 640, 400, 30, false).iter().any(|a| a == "pulse"), "no audio output when asked");
        let list = Input { path: PathBuf::from("/x/list.txt"), concat: true };
        let a = ffmpeg_args(&list, 0, 640, 400, 30, true);
        assert!(a.windows(2).any(|w| w == ["-f", "concat"]) && a.windows(2).any(|w| w == ["-safe", "0"]));
    }
}
