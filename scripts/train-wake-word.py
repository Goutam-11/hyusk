#!/usr/bin/env python3
"""
train-wake-word.py - train a personal wake word for hyusk_agent.

Records clips of YOU saying a wake word (e.g. "ragna" or "alexa"), extracts
the same audio features the Rust runtime uses, trains a small classifier head,
and exports a drop-in model:

    models/hey_ragna.onnx

The exported model plugs straight into the existing livekit-wakeword pipeline.
After training, set WAKE_WORD_MODEL=models/hey_ragna.onnx (the detector also
auto-discovers models/hey_*.onnx).

Example:
    python3 scripts/train-wake-word.py --word ragna --positives 40 --negatives 40

Dependencies (installed on demand): numpy, sounddevice, onnxruntime, onnx,
torch. Use --skip-install to disable automatic installation.

The feature extraction (mel spectrogram + speech embedding) uses the exact
models embedded in the livekit-wakeword crate, so training features match
runtime features. The classifier architecture mirrors OpenWakeWord's:
Flatten -> Linear(64) -> LayerNorm -> ReLU -> Linear(64) -> LayerNorm -> ReLU
-> Linear(1) -> Sigmoid, exported with input "embeddings" and output "score".
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import urllib.request
import wave
from pathlib import Path

import numpy as np

TARGET_SAMPLE_RATE = 16000
CLIP_SAMPLES = TARGET_SAMPLE_RATE * 2          # the runtime scores ~2 s windows
FEATURE_URLS = {
    "melspectrogram.onnx": (
        "https://github.com/dscripka/openWakeWord/releases/download/"
        "v0.5.1/melspectrogram.onnx"
    ),
    "embedding_model.onnx": (
        "https://github.com/dscripka/openWakeWord/releases/download/"
        "v0.5.1/embedding_model.onnx"
    ),
}


def ensure(package, extra_index=None):
    """Import `package` or install it (with optional pip index) and retry."""
    try:
        return __import__(package)
    except ImportError:
        print(f"[train] Installing missing package: {package}")
        # PyTorch and ONNX wheels are large. Never add another copy to pip's
        # cache; this trainer is commonly run on quota-limited home folders.
        cmd = [
            sys.executable, "-m", "pip", "install", "--user",
            "--no-cache-dir", package,
        ]
        if extra_index:
            cmd += ["--index-url", extra_index]
        try:
            subprocess.check_call(cmd)
        except subprocess.CalledProcessError as error:
            version = f"{sys.version_info.major}.{sys.version_info.minor}"
            raise SystemExit(
                f"Could not install {package} for Python {version}. "
                "PyTorch wheels are usually available for Python 3.12; "
                "try /home/goutamsharma/.local/bin/python3.12 with "
                "--no-cache-dir, and clear unused pip cache if disk quota "
                "is full."
            ) from error
        return __import__(package)


def ensure_onnxruntime():
    return ensure("onnxruntime")


class FeatureExtractor:
    """Replicates the mel + embedding steps of the livekit-wakeword runtime.

    For a 2 s mono 16 kHz clip the result is the same (16, 96) embedding
    sequence that the classifier sees at runtime (last 16 windows of 76 mel
    frames, stride 8).
    """

    def __init__(self, cache_dir):
        self.cache_dir = Path(cache_dir)
        self.cache_dir.mkdir(parents=True, exist_ok=True)

        onnxruntime = ensure_onnxruntime()
        providers = ["CPUExecutionProvider"]

        self.mel_path = self._ensure_model("melspectrogram.onnx")
        self.emb_path = self._ensure_model("embedding_model.onnx")

        self.mel_session = onnxruntime.InferenceSession(
            str(self.mel_path), providers=providers
        )
        self.emb_session = onnxruntime.InferenceSession(
            str(self.emb_path), providers=providers
        )

        self.mel_input = self.mel_session.get_inputs()[0].name
        self.emb_input = self.emb_session.get_inputs()[0].name

    def _ensure_model(self, name):
        path = self.cache_dir / name
        if path.exists():
            return path

        print(f"[train] Downloading feature model {name} ...")
        urllib.request.urlretrieve(FEATURE_URLS[name], path)
        return path

    def extract(self, audio):
        """audio: float32 numpy array (32000,) in [-1, 1]. -> (16, 96)."""
        onnxruntime = ensure_onnxruntime()

        # Match the live detector: remove sub-vocal rumble, then apply only a
        # gentle gain. The old trainer allowed 8x while runtime allowed 2x,
        # which made training and live background noise look very different.
        audio = high_pass(audio)
        peak = np.max(np.abs(audio))
        if peak > 1e-6:
            audio = audio * min(0.7 / peak, 2.0)

        samples = audio.astype(np.float32).reshape(1, -1)
        mel_raw = self.mel_session.run(None, {self.mel_input: samples})[0]
        mel = np.asarray(mel_raw)[0, 0].astype(np.float32)      # (frames, 32)
        mel = mel / 10.0 + 2.0                                   # melspec_transform

        embeddings = []
        start = 0
        while start + 76 <= mel.shape[0]:
            window = mel[start : start + 76].reshape(1, 76, 32, 1)
            emb = self.emb_session.run(None, {self.emb_input: window})[0]
            embeddings.append(np.asarray(emb)[0, 0, 0])          # (96,)
            start += 8

        if len(embeddings) < 16:
            raise ValueError(
                f"clip produced only {len(embeddings)} embeddings; "
                "expected at least 16 (use ~2 s of audio)"
            )

        return np.stack(embeddings[-16:], axis=0).astype(np.float32)


def load_wav_f32(path):
    """Read a mono 16-bit PCM WAV and return 16 kHz float32 samples."""
    with wave.open(str(path), "rb") as wav:
        if wav.getnchannels() != 1 or wav.getsampwidth() != 2:
            raise ValueError(
                f"{path}: expected mono 16-bit PCM (got {wav.getnchannels()} "
                f"channels, {wav.getsampwidth()} bytes)"
            )
        rate = wav.getframerate()
        raw = wav.readframes(wav.getnframes())
    data = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0
    return resample_linear(data, rate)


def resample_linear(data, input_rate):
    if input_rate == TARGET_SAMPLE_RATE:
        return data
    ratio = input_rate / TARGET_SAMPLE_RATE
    indices = np.arange(int(len(data) / ratio)) * ratio
    return np.interp(indices, np.arange(len(data)), data)


def to_clip_slice(audio):
    """Pad or truncate to exactly CLIP_SAMPLES at 16 kHz (silence-padded)."""
    if audio.size >= CLIP_SAMPLES:
        return audio[:CLIP_SAMPLES]
    out = np.zeros(CLIP_SAMPLES, dtype=np.float32)
    out[: audio.size] = audio
    return out


def high_pass(audio):
    """Match the runtime's one-pole 90 Hz speech-band high-pass filter."""
    audio = np.asarray(audio, dtype=np.float32)
    if not audio.size:
        return audio
    dt = 1.0 / TARGET_SAMPLE_RATE
    rc = 1.0 / (2.0 * np.pi * 90.0)
    alpha = rc / (rc + dt)
    output = np.empty_like(audio)
    previous_input = float(audio[0])
    previous_output = 0.0
    for index, sample in enumerate(audio):
        filtered = alpha * (previous_output + float(sample) - previous_input)
        output[index] = filtered
        previous_input = float(sample)
        previous_output = filtered
    return output


def record_clip(device=None):
    """Record one clip via sounddevice and return float32 16 kHz mono audio."""
    sounddevice = ensure("sounddevice")

    print(">>> Recording now. Say the wake word once and stay quiet. <<<")
    duration = 2.0
    data = sounddevice.rec(
        int(duration * TARGET_SAMPLE_RATE),
        samplerate=TARGET_SAMPLE_RATE,
        channels=1,
        dtype="float32",
        device=device,
    )
    sounddevice.wait()
    audio = data[:, 0] if data.ndim > 1 else data
    return np.asarray(audio, dtype=np.float32)


def save_wav(path, audio):
    """Persist a float32 clip as mono 16-bit PCM for resumable training."""
    path.parent.mkdir(parents=True, exist_ok=True)
    samples = np.clip(audio, -1.0, 1.0)
    pcm = (samples * 32767.0).astype(np.int16)
    with wave.open(str(path), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(TARGET_SAMPLE_RATE)
        wav.writeframes(pcm.tobytes())


def collect_clips(word, count, message, device=None, output_dir=None):
    """Interactively record `count` clips of the user speaking `word`."""
    clips = load_clip_dir(output_dir) if output_dir else []
    start = len(clips)
    if start:
        print(f"[train] resuming {message}: found {start}/{count} saved clips")

    for i in range(start, count):
        input(
            f"[{i + 1}/{count}] press Enter to record \"{word}\" "
            f"({message}) ..."
        )

        while True:
            audio = record_clip(device)
            peak = float(np.max(np.abs(audio)))
            if peak < 0.02:
                print("  Too quiet, recording again. Speak louder.")
            else:
                clips.append(audio)
                if output_dir:
                    save_wav(output_dir / f"{i + 1:04d}.wav", audio)
                print(f"  ✓ clip {i + 1} (peak {peak:.2f})")
                break

    print(f"Collected {len(clips)} {message}.")
    return clips


def record_negatives(count, device=None, output_dir=None):
    """Record `count` clips of ambient noise / unrelated speech."""
    clips = load_clip_dir(output_dir) if output_dir else []
    start = len(clips)
    if start:
        print(f"[train] resuming negatives: found {start}/{count} saved clips")

    for i in range(start, count):
        prompts = [
            "say confusing phrases such as hey Siri, hey Google, Alexa, hey you, hi Hyusk, or hey Musk",
            "speak normal conversation without saying Hey Hyusk",
            "play the usual background TV, music, fan, keyboard, or room noise",
        ]
        input(f"Negative {i + 1}/{count}: press Enter, then {prompts[i % len(prompts)]}.")
        audio = record_clip(device)
        clips.append(audio)
        if output_dir:
            save_wav(output_dir / f"{i + 1:04d}.wav", audio)
        print(f"  ✓ negative {i + 1}")

    return clips


def augment_positive(audio, negatives, rng):
    """Time-jitter a positive clip and optionally mix in a noise clip."""
    # Keep the whole recording. The old augmentation copied only its first
    # second, sometimes cutting off the wake word or its final syllable.
    positive = shift_clip(to_clip_slice(audio), int(rng.uniform(-4000, 4000)))

    if negatives:
        # The recordings can have different lengths, so passing the list of
        # arrays directly to ``RandomState.choice`` makes NumPy try to build a
        # rectangular array and fail. Choose an index first.
        noise = negatives[int(rng.randint(len(negatives)))]
        # Normalize short/long recordings to the runtime's fixed two-second
        # window before mixing, otherwise NumPy broadcasting fails on a short
        # WAV file.
        target = to_clip_slice(noise)
        snr_db = rng.uniform(8.0, 24.0)
        signal = positive[: target.size]
        signal_power = np.mean(signal**2) + 1e-12
        noise_power = np.mean(target**2) + 1e-12
        gain = np.sqrt(signal_power / noise_power) * (10.0 ** (-snr_db / 20.0))
        return positive + target * gain
    return positive


def shift_clip(audio, offset):
    """Move a fixed-length clip without wrapping its edge into the signal."""
    source = to_clip_slice(audio)
    shifted = np.zeros(CLIP_SAMPLES, dtype=np.float32)
    if offset >= 0:
        shifted[offset:] = source[:CLIP_SAMPLES - offset]
    else:
        shifted[:offset] = source[-offset:]
    return shifted


def augment_negative(audio, negatives, rng):
    """Vary hard-negative timing and mix in a little real room background."""
    result = shift_clip(audio, int(rng.uniform(-4000, 4000)))
    if len(negatives) > 1:
        background = to_clip_slice(negatives[int(rng.randint(len(negatives)))])
        result = result + background * float(rng.uniform(0.1, 0.35))
    return np.clip(result, -1.0, 1.0)


def build_dataset(feature_extractor, positives, negatives, augment,
                  negative_augmentations=2, seed=0, verbose=True):
    """Extract (16, 96) features + labels. Augments positives when enabled."""
    rng = np.random.RandomState(seed)

    positives = [np.asarray(c, dtype=np.float32) for c in positives]
    negatives = [np.asarray(c, dtype=np.float32) for c in negatives]

    X, y = [], []

    def add(audio, label):
        X.append(feature_extractor.extract(to_clip_slice(audio)))
        y.append(label)

    for clip in positives:
        for _ in range(2 if augment else 1):
            add(augment_positive(clip, negatives, rng) if augment else clip, 1)

    for clip in negatives:
        add(clip, 0)
        if augment:
            for _ in range(negative_augmentations):
                add(augment_negative(clip, negatives, rng), 0)

    X = np.asarray(X, dtype=np.float32)
    y = np.asarray(y, dtype=np.float32).reshape(-1, 1)
    if verbose:
        print(f"[train] dataset: {X.shape[0]} clips, "
              f"{int((y == 1).sum())} positive, {int((y == 0).sum())} negative")
    return X, y


def train_model(X, y, epochs=60, seed=0):
    """Train a small MLP classifier head, mirroring OpenWakeWord's layout."""
    torch = ensure("torch")
    from torch import nn

    torch.manual_seed(seed)
    np.random.seed(seed)

    model = nn.Sequential(
        nn.Flatten(),
        nn.Linear(16 * 96, 64),
        nn.LayerNorm(64),
        nn.ReLU(),
        nn.Linear(64, 64),
        nn.LayerNorm(64),
        nn.ReLU(),
        nn.Linear(64, 1),
        nn.Sigmoid(),
    )

    X_t = torch.from_numpy(X)
    y_t = torch.from_numpy(y)

    # 80/20 split for early stopping.
    n_val = max(1, X_t.shape[0] // 5)
    perm = torch.randperm(X_t.shape[0])
    val_idx, train_idx = perm[:n_val], perm[n_val:]
    X_tr, y_tr = X_t[train_idx], y_t[train_idx]
    X_va, y_va = X_t[val_idx], y_t[val_idx]

    # The classifier includes a final Sigmoid so its exported ONNX output is
    # already a confidence score in [0, 1]. Use BCELoss here; pairing the
    # Sigmoid with BCEWithLogitsLoss would apply the logistic transform twice.
    # Keep the classes balanced even when the recommended hard-negative set is
    # much larger than the personal positive set. Otherwise adding useful
    # negatives can make the easiest solution "never wake".
    loss_fn = nn.BCELoss(reduction="none")
    positive_count = max(1, int((y_tr >= 0.5).sum().item()))
    negative_count = max(1, int((y_tr < 0.5).sum().item()))
    total_count = positive_count + negative_count
    positive_weight = total_count / (2.0 * positive_count)
    negative_weight = total_count / (2.0 * negative_count)
    optimizer = torch.optim.Adam(model.parameters(), lr=1e-3,
                                 weight_decay=1e-4)

    best_val = float("inf")
    best_state = None
    patience = 8
    stale = 0
    batch = 32

    for epoch in range(epochs):
        model.train()
        perm2 = torch.randperm(X_tr.shape[0])
        running = 0.0
        for i in range(0, X_tr.shape[0], batch):
            idx = perm2[i : i + batch]
            logits = model(X_tr[idx]).squeeze(-1)
            targets = y_tr[idx].squeeze(-1)
            weights = torch.where(
                targets >= 0.5,
                torch.full_like(targets, positive_weight),
                torch.full_like(targets, negative_weight),
            )
            loss = (loss_fn(logits, targets) * weights).mean()
            optimizer.zero_grad()
            loss.backward()
            optimizer.step()
            running += loss.item()

        model.eval()
        with torch.no_grad():
            val_loss = loss_fn(
                model(X_va).squeeze(-1), y_va.squeeze(-1)
            ).mean().item()

        if (epoch + 1) % 10 == 0 or epoch == 0:
            steps = max(1, (X_tr.shape[0] - 1) // batch + 1)
            print(f"[train] epoch {epoch + 1:>3}/{epochs} "
                  f"train {running / steps:.4f} val {val_loss:.4f}")

        if val_loss < best_val:
            best_val = val_loss
            best_state = {
                k: v.detach().clone() for k, v in model.state_dict().items()
            }
            stale = 0
        else:
            stale += 1
            if stale >= patience:
                print(f"[train] early stop at epoch {epoch + 1}")
                break

    model.load_state_dict(best_state)

    # Report the score separation on the whole dataset.
    model.eval()
    with torch.no_grad():
        scores = model(X_t).numpy()[:, 0]
    pos = scores[y[:, 0] == 1]
    neg = scores[y[:, 0] == 0]
    if pos.size and neg.size:
        mid = (pos.mean() + neg.mean()) / 2
        print(
            f"[train] positive scores: {pos.mean():.3f} +/- {pos.std():.3f} | "
            f"negative scores: {neg.mean():.3f} +/- {neg.std():.3f}"
        )
        print(f"[train] suggested threshold ~ {max(0.1, min(0.9, mid)):.2f}")

    return model
def export_model(model, output_path, word):
    """Export the trained head as a livekit-wakeword compatible ONNX model."""
    torch = ensure("torch")
    onnx = ensure("onnx")

    output_path.parent.mkdir(parents=True, exist_ok=True)

    model = model.cpu().eval()
    dummy = torch.randn(1, 16, 96)

    torch.onnx.export(
        model,
        dummy,
        str(output_path),
        input_names=["embeddings"],
        output_names=["score"],
        # LiveKit/OpenWakeWord expects the original fixed classifier contract:
        # embeddings [1, 16, 96] -> score [1, 1], opset 13.
        opset_version=13,
        dynamo=False,
    )

    onnx.checker.check_model(onnx.load(str(output_path)))

    # Sanity check with onnxruntime.
    onnxruntime = ensure_onnxruntime()
    sess = onnxruntime.InferenceSession(
        str(output_path), providers=["CPUExecutionProvider"]
    )
    out = sess.run(
        ["score"],
        {"embeddings": np.random.rand(1, 16, 96).astype(np.float32)},
    )
    score = float(np.asarray(out[0]).reshape(-1)[0])
    if not 0.0 <= score <= 1.0:
        raise ValueError(f"exported model returned out-of-range score {score}")

    print(f"[train] ✓ Wrote model: {output_path}")
    print(
        f"[train]   wake word=\"{word}\", input=embeddings(1,16,96), "
        "output=score"
    )


def load_clip_dir(directory, recursive=False, limit=None, seed=0, quiet=False):
    """Load WAV clips from a directory, optionally sampling a bounded subset."""
    clips = []
    root = Path(directory)
    paths = sorted(root.rglob("*.wav") if recursive else root.glob("*.wav"))
    if limit is not None and len(paths) > limit:
        rng = np.random.RandomState(seed)
        selected = sorted(rng.choice(len(paths), size=limit, replace=False))
        paths = [paths[index] for index in selected]
    for path in paths:
        try:
            clips.append(load_wav_f32(path))
            if not quiet:
                print(f"[train] loaded {path.name}")
        except ValueError as error:
            print(f"[train] skipping {path.name}: {error}")
    if quiet:
        print(f"[train] loaded {len(clips)} public negative clips from {root}")
    return clips


def parse_args():
    parser = argparse.ArgumentParser(
        description="Train a personal wake word for hyusk_agent."
    )
    parser.add_argument("--word", default="ragna",
                        help="wake word to train (default: ragna)")
    parser.add_argument(
        "--output", default=None,
        help="output .onnx path (default: models/hey_<word>.onnx)"
    )
    parser.add_argument(
        "--positives", type=int, default=40,
        help="number of recordings of your voice saying the word "
             "(default: 40)"
    )
    parser.add_argument(
        "--negatives", type=int, default=40,
        help="number of noise/unrelated-speech recordings (default: 40)"
    )
    parser.add_argument(
        "--positives-dir", default=None,
        help="load positive clips from a folder of mono 16-bit WAV files "
             "instead of recording"
    )
    parser.add_argument(
        "--negatives-dir", default=None,
        help="load negative clips from a folder of mono 16-bit WAV files "
             "instead of recording"
    )
    parser.add_argument(
        "--extra-negatives-dir", action="append", default=[],
        help="add a public/external WAV tree to the personal negatives; may "
             "be repeated"
    )
    parser.add_argument(
        "--extra-negative-limit", type=int, default=2000,
        help="deterministic clip sample from each extra tree (default: 2000)"
    )
    parser.add_argument(
        "--recordings-dir", default="models/wake_recordings",
        help="checkpoint recordings here and resume interrupted sessions"
    )
    parser.add_argument("--epochs", type=int, default=60)
    parser.add_argument(
        "--no-augment", action="store_true",
        help="disable time-jitter/noise-mix augmentation"
    )
    parser.add_argument(
        "--negative-augmentations", type=int, default=2,
        help="timing/background variants per negative clip (default: 2)"
    )
    parser.add_argument(
        "--feature-models-dir", default="models/feature_extraction",
        help="where to cache the mel/embedding feature models"
    )
    parser.add_argument(
        "--device", default=None,
        help="sounddevice input device index/name for recording"
    )
    parser.add_argument(
        "--skip-install", action="store_true",
        help="do not auto-install python packages"
    )
    return parser.parse_args()


def main():
    args = parse_args()

    if args.skip_install:
        def ensure(package, *a, **k):
            try:
                return __import__(package)
            except ImportError:
                raise SystemExit(
                    f"Missing python package: {package}. Run without "
                    "--skip-install or install it manually."
                )
        globals()["ensure"] = ensure

    cache_dir = Path(args.feature_models_dir)
    extractor = FeatureExtractor(cache_dir)
    word = args.word.strip().lower().replace(" ", "_")
    recordings_root = Path(args.recordings_dir) / word
    positive_recordings = recordings_root / "positives"
    negative_recordings = recordings_root / "negatives"

    # 1) Positives
    if args.positives_dir:
        positives = load_clip_dir(args.positives_dir)
    else:
        print(f"\n=== Recording {args.positives} positive clips ===\n")
        positives = collect_clips(args.word, args.positives, "positives",
                                  device=args.device, output_dir=positive_recordings)

    # 2) Negatives
    if args.negatives_dir:
        negatives = load_clip_dir(args.negatives_dir)
    elif args.negatives and not args.positives_dir:
        print("\n=== Recording negatives (background/other speech) ===\n")
        negatives = record_negatives(args.negatives, device=args.device,
                                     output_dir=negative_recordings)
    else:
        negatives = []

    for index, directory in enumerate(args.extra_negatives_dir):
        negatives.extend(load_clip_dir(
            directory,
            recursive=True,
            limit=max(1, args.extra_negative_limit),
            seed=index,
            quiet=True,
        ))

    output = Path(args.output or f"models/hey_{word}.onnx")

    X, y = build_dataset(
        extractor,
        positives,
        negatives,
        augment=not args.no_augment and bool(negatives),
        negative_augmentations=max(0, args.negative_augmentations),
    )
    if len(X) < 4:
        raise SystemExit("not enough clips to train (need at least a few)")

    model = train_model(X, y, epochs=args.epochs)
    export_model(model, output, word)

    print()
    print(f"All done! Enable your new wake word with:")
    print(f"  export WAKE_WORD_MODEL={output}")
    print("  export WAKE_WORD_THRESHOLD=0.5   # tune after testing")
    print("Then run hyusk and say the word. More test clips improve accuracy.")


if __name__ == "__main__":
    main()
