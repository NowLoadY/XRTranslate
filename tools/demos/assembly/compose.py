"""Assemble an ordered picture track and independently placed sound tracks."""

from pathlib import Path

from .media import run, scan, sidecars, write_json


def anchor(value, shots):
    if isinstance(value, (int, float)):
        return value
    name, edge = value.rsplit(".", 1)
    shot = next(s for s in shots if s["id"] == name)
    if edge not in ("start", "end"):
        raise ValueError(f"Unknown timeline anchor: {value}")
    return shot["start"] + (shot["duration"] if edge == "end" else 0)


def assemble(ffmpeg, shots, sounds, project, version, output):
    output.mkdir(parents=True, exist_ok=True)
    destination = output / project.get("file", f"{version['id']}.mp4")
    products = {
        destination.resolve(),
        (output / "captions.srt").resolve(),
        (output / "chapters.ffmeta").resolve(),
    }
    if any(
        Path(s[key]).resolve() in products
        for s in shots
        for key in ("file", "captions", "chapters")
        if s.get(key)
    ):
        raise ValueError("Output must preserve the source videos and sidecars.")
    staging = Path(project["work"]) / version["id"] / "encode"
    staging.mkdir(parents=True, exist_ok=True)
    target = staging / destination.name
    fps, (width, height) = project["fps"], project["size"]
    total_frames = sum(s["frames"] for s in shots)
    duration = total_frames / fps
    subtitles, metadata = sidecars(shots, staging, project["title"])
    command = [ffmpeg, "-hide_banner", "-v", "error", "-y"]
    filters, pair = [], []
    for i, shot in enumerate(shots):
        command += ["-i", shot["file"]]
        trim = f"start={shot['in']}:duration={shot['duration']}"
        video = f"[{i}:v]trim={trim},setpts=PTS-STARTPTS,fps={fps}"
        hold = round(shot.get("opening_hold", 0) * fps)
        if hold:
            video += f",trim=start_frame={hold},setpts=PTS-STARTPTS,fps={fps},tpad=start={hold}:start_mode=clone"
        video += (
            f",scale={width}:{height}:force_original_aspect_ratio=decrease,"
            f"pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color={project['background']},"
            f"setsar=1,settb=1/60000,trim=end_frame={shot['frames']}[v{i}]"
        )
        if shot["audio"]:
            audio = f"[{i}:a]atrim={trim},asetpts=PTS-STARTPTS,aresample=48000,aformat=channel_layouts=stereo"
            audio += f",volume={shot.get('gain_db', 0)}dB"
        else:
            audio = "anullsrc=r=48000:cl=stereo"
        audio += f",apad=whole_dur={shot['duration']},atrim=duration={shot['duration']}[a{i}]"
        filters += [video, audio]
        pair.append(f"[v{i}][a{i}]")
    filters.append("".join(pair) + f"concat=n={len(shots)}:v=1:a=1[video][base]")

    ducked = [i for i, sound in enumerate(sounds) if sound.get("duck", False)]
    if ducked:
        filters.append(
            f"[base]asplit={len(ducked)+1}[voice]"
            + "".join(f"[control{i}]" for i in ducked)
        )
    else:
        filters.append("[base]anull[voice]")
    for i, sound in enumerate(sounds):
        if sound.get("loop", False):
            command += ["-stream_loop", "-1"]
        command += ["-i", sound["file"]]
        begin = anchor(sound.get("start", 0), shots)
        end = anchor(sound.get("end", duration), shots)
        if not 0 <= begin < end <= duration + 0.001:
            raise ValueError(
                f"Sound track lies outside the picture track: {sound['file']}"
            )
        length = end - begin
        fade_in, fade_out = sound.get("fade_in", 0), sound.get("fade_out", 0)
        bed = (
            f"[{len(shots)+i}:a]atrim=start={sound.get('in', 0)}:duration={length},"
            "asetpts=PTS-STARTPTS,aresample=48000,aformat=channel_layouts=stereo,"
            f"volume={sound.get('gain_db', 0)}dB"
        )
        if fade_in:
            bed += f",afade=t=in:d={min(fade_in,length)}"
        if fade_out:
            bed += f",afade=t=out:st={max(0,length-fade_out)}:d={min(fade_out,length)}"
        delay = round(begin * 1000)
        bed += f",adelay={delay}|{delay},apad=whole_dur={duration},atrim=duration={duration}[bed{i}]"
        filters.append(bed)
        if i in ducked:
            filters.append(
                f"[bed{i}][control{i}]sidechaincompress=threshold=0.07:ratio=3:attack=20:release=450:detection=rms[sound{i}]"
            )
        else:
            filters.append(f"[bed{i}]anull[sound{i}]")
    audio = "[voice]" + "".join(f"[sound{i}]" for i in range(len(sounds)))
    filters.append(
        audio
        + f"amix=inputs={len(sounds)+1}:duration=first:normalize=0,alimiter=limit=0.95:level=0:latency=1[audio]"
    )

    index = len(shots) + len(sounds)
    subtitle_index = index if subtitles else None
    if subtitles:
        command += ["-i", subtitles]
        index += 1
    command += [
        "-i",
        metadata,
        "-filter_complex_threads",
        "1",
        "-filter_complex",
        ";".join(filters),
        "-map",
        "[video]",
        "-map",
        "[audio]",
        "-map_metadata",
        index,
        "-map_chapters",
        index,
    ]
    if subtitles:
        command += [
            "-map",
            f"{subtitle_index}:0",
            "-c:s",
            "mov_text",
            "-disposition:s:0",
            "default",
            "-metadata:s:s:0",
            f"language={version['language']}",
            "-metadata:s:s:0",
            f"title={version['subtitle_title']}",
        ]
    command += [
        "-c:v",
        "libx264",
        "-preset",
        "fast",
        "-crf",
        "17",
        "-threads",
        "4",
        "-pix_fmt",
        "yuv420p",
        "-video_track_timescale",
        "15360",
        "-c:a",
        "aac",
        "-b:a",
        "160k",
        "-metadata:s:a:0",
        f"language={version['language']}",
        "-movflags",
        "+faststart",
        target,
    ]
    run(command)
    verified = scan(ffmpeg, target, decode=True)
    if verified["frames"] != total_frames:
        raise ValueError(
            f"Unexpected output length: {verified['frames']} / {total_frames}"
        )
    verified.update(
        file=target.name,
        duration=duration,
        bytes=target.stat().st_size,
        full_decode="passed",
    )
    write_json(staging / "verified.json", verified)
    write_json(
        staging / "composition.json",
        {"shots": shots, "audio": sounds, "fps": fps, "duration": duration},
    )
    for file in (
        target,
        staging / "captions.srt",
        metadata,
        staging / "verified.json",
        staging / "composition.json",
    ):
        file.replace(output / file.name)
    print(f"{version['id']}: {destination} ({duration:.2f}s)", flush=True)
    return destination
