import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

from .common import ROOT, digest, save


def detection_snapshot(work):
    return {str(p): [p.stat().st_mtime_ns, digest(p)] for p in work.glob("*/**/*.complete.json") if p.name in ("normalize.complete.json", "ball.complete.json", "players.complete.json") and "renders" not in p.parts}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    base = [sys.executable, "-m", "volleyball_studio", "--non-interactive", "--input", str(args.input), "--work", str(args.work), "--analysis-seconds", "12", "--duration", "5", "--min-rally", "2", "--width", "360"]
    report = {"cases": []}

    def execute(name, extra):
        destination = args.output / name
        command = base + ["--output", str(destination)] + extra
        started = time.monotonic()
        with (args.output / f"{name}.log").open("w") as log:
            result = subprocess.run(command, stdout=log, stderr=log, cwd=ROOT)
        if result.returncode:
            raise RuntimeError(f"{name}: код {result.returncode}, см. журнал")
        run_dir = max(destination.iterdir(), key=lambda p: p.stat().st_mtime_ns)
        report["cases"].append({"name": name, "seconds": time.monotonic()-started, "result": str(run_dir), "exit_code": result.returncode})
        save(args.output / "acceptance.json", report)
        return run_dir, command

    first, command = execute("classic", ["--style", "classic", "--music", "no"])
    before = detection_snapshot(args.work)
    second, _ = execute("cinematic", ["--style", "cinematic", "--music", "yes"])
    after = detection_snapshot(args.work)
    if before != after:
        raise RuntimeError("Смена стиля изменила завершённую детекцию")
    report["style_change_keeps_detection"] = True
    selected = json.loads((second / "previews/highlights.json").read_text())
    selection = args.output / "manual-selection.json"
    save(selection, {"include": [selected[0]["id"]], "exclude": []})
    execute("manual", ["--style", "dynamic", "--selection-file", str(selection)])
    file = next((first / "renders/classic").glob("*.mp4"))
    fingerprint = digest(file)
    with (args.output / "resume.log").open("w") as log:
        resumed = subprocess.run(command + ["--resume", str(first)], stdout=log, stderr=log, cwd=ROOT)
    if resumed.returncode or digest(file) != fingerprint:
        raise RuntimeError("Восстановление изменило готовый MP4")
    report["resume_preserves_output"] = True
    if os.name != "nt":
        interrupted_out = args.output / "interrupted"
        interrupted_command = base + ["--style", "cinematic", "--width", "1080", "--output", str(interrupted_out)]
        with (args.output / "interrupted.log").open("w") as log:
            process = subprocess.Popen(interrupted_command, stdout=log, stderr=log, cwd=ROOT)
            deadline = time.monotonic()+120
            sent = False
            while process.poll() is None and time.monotonic() < deadline:
                if list(interrupted_out.glob("*/reports/D.status.json")):
                    process.send_signal(signal.SIGINT)
                    sent = True
                    break
                time.sleep(.05)
            if not sent:
                if process.poll() is None:
                    process.terminate()
                process.wait()
                raise RuntimeError("Не удалось прервать процесс на стадии монтажа")
            process.wait(timeout=60)
        interrupted_run = next(interrupted_out.iterdir())
        manifest = json.loads((interrupted_run / "manifest.json").read_text())
        if manifest["status"] != "interrupted":
            raise RuntimeError("Прерванный запуск не отмечен interrupted")
        with (args.output / "recovery.log").open("w") as log:
            recovered = subprocess.run(interrupted_command + ["--resume", str(interrupted_run)], stdout=log, stderr=log, cwd=ROOT)
        if recovered.returncode or before != detection_snapshot(args.work):
            raise RuntimeError("Восстановление после SIGINT не прошло")
        report["sigint_recovered_without_detection"] = True
        report["recovered_demo"] = str(next((interrupted_run / "renders/cinematic").glob("*.mp4")))
    report["passed"] = True
    save(args.output / "acceptance.json", report)
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
