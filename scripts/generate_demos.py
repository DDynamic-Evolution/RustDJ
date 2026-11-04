"""Generate two original 120 BPM demo loops; Python standard library only."""
from pathlib import Path
import math
import random
import struct
import wave

rate = 44100
duration = 8
root = Path(__file__).resolve().parent.parent / "assets"
root.mkdir(exist_ok=True)
for deck, notes in [("a", [55, 65.406, 73.416, 65.406]), ("b", [110, 130.813, 146.832, 164.814])]:
    rng = random.Random(400 if deck == "a" else 900)
    frames = bytearray()
    for i in range(rate * duration):
        t = i / rate
        beat = t % 0.5
        kick = math.sin(2 * math.pi * (48 * beat + 5 * (1 - math.exp(-30 * beat)))) * math.exp(-19 * beat)
        step = t % 0.25
        noise = rng.uniform(-1, 1)
        hat = noise * math.exp(-100 * step) * 0.13
        snare = noise * math.exp(-28 * beat) * 0.18 if int(t / 0.5) % 2 else 0
        frequency = notes[int(t / 2) % 4]
        bass = (math.sin(2 * math.pi * frequency * t) + 0.25 * math.sin(4 * math.pi * frequency * t)) * math.exp(-4 * beat) * 0.2
        arp_freq = frequency * [4, 6, 8, 12][int(t / 0.25) % 4]
        arp = math.sin(2 * math.pi * arp_freq * t) * math.exp(-15 * step) * (0.09 if deck == "a" else 0.13)
        envelope = min(t / 0.01, 1, (duration - t) / 0.02)
        sample = (kick * 0.38 + hat + snare + bass + arp) * envelope * 0.7
        left = max(-1, min(1, sample))
        right = max(-1, min(1, sample + arp * 0.15 * math.sin(t * 2)))
        frames += struct.pack('<hh', int(left * 32767), int(right * 32767))
    with wave.open(str(root / f"demo-{deck}.wav"), 'wb') as out:
        out.setnchannels(2)
        out.setsampwidth(2)
        out.setframerate(rate)
        out.writeframes(frames)
    print(root / f"demo-{deck}.wav")
