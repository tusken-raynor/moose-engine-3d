"""Generate the test sounds in assets/sounds: step.wav (a footstep: a soft, short pat),
land.wav (landing from a jump or fall: a thump) and test_hum.wav (a hum that loops
without a seam, with pitches from low to high so muffling can be heard).

Usage (from repo root): python3 tools/gen_test_sounds.py .

All are 16-bit mono at 22050 Hz. They are placeholders made from sine waves and noise.
"""
import math, os, random, struct, sys, wave

ROOT = sys.argv[1]
RATE = 22050


def write(name, samples, level=0.9):
    """Writes samples (floats, scaled so the loudest is at `level` of full) as assets/sounds/name."""
    peak = max(abs(x) for x in samples) or 1.0
    path = os.path.join(ROOT, "assets", "sounds", name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with wave.open(path, "wb") as f:
        f.setnchannels(1)
        f.setsampwidth(2)
        f.setframerate(RATE)
        f.writeframes(b"".join(struct.pack("<h", round(x / peak * level * 32767)) for x in samples))
    print(f"wrote {path}: {len(samples) / RATE:.2f} s")


def step():
    """A footstep: a soft pat, 70 ms long. Mostly noise low-passed twice over (to about
    1 kHz, so it is dull, not a hiss or a click), swelling over 4 ms and dying away fast,
    on a faint low tone that gives it a little weight."""
    rng = random.Random(3)
    n = int(RATE * 0.07)
    out, low, lower = [], 0.0, 0.0
    for i in range(n):
        t = i / RATE
        low += 0.25 * (rng.uniform(-1, 1) - low)
        lower += 0.25 * (low - lower)
        pat = lower * math.exp(-t * 75)
        weight = math.sin(math.tau * 110 * t) * math.exp(-t * 60) * 0.12
        # Faded in, and out over the last few ms, so it neither starts nor ends on a click.
        out.append((pat + weight) * min(1.0, i / (RATE * 0.004)) * min(1.0, (n - i) / (RATE * 0.01)))
    return out


def land():
    """Landing: a low thump that drops in pitch, and a short burst of noise over it (the
    scuff), low-passed so it isn't a hiss."""
    rng = random.Random(1)
    n = int(RATE * 0.16)
    out, low, phase = [], 0.0, 0.0
    for i in range(n):
        t = i / RATE
        phase += math.tau * (55 + 90 * math.exp(-t * 40)) / RATE
        thump = math.sin(phase) * math.exp(-t * 28)
        low += 0.25 * (rng.uniform(-1, 1) - low)
        scuff = low * math.exp(-t * 60) * 0.9
        # A few samples of fade-in, so it doesn't start with a click.
        out.append((thump + scuff) * min(1.0, i / 20))
    return out


def hum():
    """A machine hum, two seconds long: a 55 Hz tone and its overtones, and many faint
    higher pitches up to 8 kHz. Every pitch fits the loop a whole number of times, so the
    end runs into the start."""
    rng = random.Random(2)
    seconds = 2
    n = RATE * seconds
    parts = [(55 * k, 1.0 / k, rng.uniform(0, math.tau)) for k in range(1, 9)]
    # (Pitches in half-Hz steps fit two seconds exactly.)
    parts += [(rng.randrange(600, 16000) / 2, 0.035, rng.uniform(0, math.tau)) for _ in range(60)]
    return [sum(a * math.sin(math.tau * hz * i / RATE + ph) for hz, a, ph in parts) for i in range(n)]


write("step.wav", step(), 0.4)
write("land.wav", land())
write("test_hum.wav", hum())
