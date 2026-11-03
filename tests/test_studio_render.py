import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))

from volleyball_studio.common import encoder, probe, run
from volleyball_studio.render import assemble, render_segment


@unittest.skipUnless(shutil.which("ffmpeg"), "FFmpeg required")
class RenderIntegration(unittest.TestCase):
    def test_music_ducking_reduces_music_during_match_sound(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            source, music = folder / "match.mp4", folder / "music.wav"
            run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=180x320:r=30:d=2", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2", "-af", "volume=0:enable='lt(t,0.5)+gt(t,1.0)'", "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac", source])
            run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=220:sample_rate=48000:duration=2", music])
            args = SimpleNamespace(duration=2., match_volume=1., music_volume=1., accent_volume=0., accents=False, effects="off", ducking=False)
            amplitudes = []
            for enabled in (False, True):
                args.ducking = enabled
                result = folder / f"duck_{enabled}.mp4"
                assemble([source], result, music, args, encoder("cpu"), [])
                decoded = subprocess.run(["ffmpeg", "-v", "error", "-i", str(result), "-vn", "-ar", "48000", "-ac", "1", "-f", "f32le", "pipe:1"], capture_output=True, check=True)
                samples = np.frombuffer(decoded.stdout, dtype="<f4")[int(.7*48000):int(.9*48000)]
                t = np.arange(len(samples))/48000
                amplitudes.append(abs(np.sum(samples*np.exp(-2j*np.pi*220*t)))/len(samples))
            self.assertLess(amplitudes[1], amplitudes[0]*.8)

    def test_cpu_slow_audio_silent_source_music_and_transition(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            source = folder / "with_audio.mp4"
            silent = folder / "silent.mp4"
            music = folder / "music.wav"
            run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=640x360:r=30:d=2", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2", "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac", "-shortest", source])
            run(["ffmpeg", "-v", "error", "-y", "-i", source, "-an", "-c:v", "copy", silent])
            run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=220:sample_rate=48000:duration=1", music])
            args = SimpleNamespace(width=180, effects="cinematic", min_confidence=.65, duration=5., music_volume=.1, match_volume=1., accent_volume=.5, accents=True)
            item = {"start": 0, "end": 59, "analysis_offset": 0., "analysis_width": 640, "analysis_height": 360, "track": {"positions": [[[300+f, 140], f] for f in range(60)]}, "ball": {f: {"x": 300+f, "y": 140, "confidence": .99} for f in range(60)}, "ball_rows": {}, "players": {}, "source": str(source), "contacts": [{"frame": 10, "contact_type": "attack", "confidence": .99, "speed_after_px_frame": 20}]}
            codec = encoder("cpu")
            first = folder / "first.mp4"
            duration, effects = render_segment(item, first, args, codec)
            timeline = json.loads(first.with_suffix(".timeline.json").read_text())
            self.assertTrue(any(e["fire"] for e in effects))
            self.assertTrue(any(row["repeat"] == 1 for row in timeline["mapping"]))
            self.assertAlmostEqual(duration, 91/30, places=3)
            media = probe(first)
            audio_duration = float(next(s for s in media["streams"] if s["codec_type"] == "audio")["duration"])
            self.assertLess(abs(audio_duration-duration), .08)
            args.speed_ramp = True
            args.style = "cinematic"
            args.grade = True
            args.titles = True
            ramped = folder / "ramped.mp4"
            ramp_duration, _ = render_segment(item, ramped, args, codec)
            ramp_map = json.loads(ramped.with_suffix(".timeline.json").read_text())
            self.assertTrue(any(r["speed"] == .5 for r in ramp_map["audio_runs"]))
            self.assertTrue(any(r["speed"] == 1.5 for r in ramp_map["audio_runs"]))
            ramp_audio = next(s for s in probe(ramped)["streams"] if s["codec_type"] == "audio")
            self.assertLess(abs(float(ramp_audio["duration"])-ramp_duration), .08)
            item["source"] = str(silent)
            item["ball"] = {}
            args.effects = "off"
            second = folder / "second.mp4"
            _, effects_off = render_segment(item, second, args, codec)
            self.assertEqual(effects_off, [])
            self.assertFalse(json.loads(second.with_suffix(".timeline.json").read_text())["original_audio"])
            self.assertTrue(all(not row.get("overview") for row in json.loads(second.with_suffix(".timeline.json").read_text())["mapping"]))
            args.effects = "cinematic"
            final = folder / "final.mp4"
            result = assemble([first, second], final, music, args, codec, [.4])
            self.assertLess(abs(float(result["format"]["duration"]) - (duration+2)), .1)


if __name__ == "__main__":
    unittest.main()
