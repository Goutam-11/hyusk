#!/usr/bin/env python3
"""
convert-openwakeword-model.py - make an OpenWakeWord ONNX classifier loadable
by the `livekit-wakeword` engine used by hyusk_agent.

The Rust runtime runs each classifier session with an input named "embeddings"
(shape (1, 16, 96)) and reads the "score" output. OpenWakeWord exports its
classifiers with generic torch names (e.g. "onnx::Flatten_0" -> "13"), so the
graph input/output names are rewritten to match.

Usage:
    python3 scripts/convert-openwakeword-model.py alexa_v0.1.onnx models/alexa.onnx
"""
import argparse
import subprocess
import sys


def ensure_onnx():
    try:
        import onnx
        return onnx
    except ImportError:
        print("The 'onnx' python package is required. Installing it...")
        subprocess.check_call(
            [sys.executable, "-m", "pip", "install", "--user", "onnx"]
        )
        import onnx
        return onnx


def main():
    ap = argparse.ArgumentParser(
        description="Rename an OpenWakeWord ONNX classifier to the "
        "livekit-wakeword embeddings->score interface."
    )
    ap.add_argument("input", help="OpenWakeWord-exported classifier (.onnx)")
    ap.add_argument("output", help="Destination livekit-wakeword model (.onnx)")
    ap.add_argument("--input-name", default="embeddings")
    ap.add_argument("--output-name", default="score")
    args = ap.parse_args()

    onnx = ensure_onnx()

    model = onnx.load(args.input)
    graph = model.graph

    # Graph outputs produced by nodes. External inputs are those NOT produced
    # by any node, i.e. the tensors the runtime must feed.
    produced = set()
    for node in graph.node:
        produced.update(node.output)

    external_inputs = [vi for vi in graph.input if vi.name not in produced]
    if len(external_inputs) != 1:
        raise SystemExit(
            "expected a classifier with exactly one external input, got "
            f"{[vi.name for vi in external_inputs]}"
        )

    renames = {external_inputs[0].name: args.input_name}
    if graph.output:
        renames[graph.output[0].name] = args.output_name
    else:
        raise SystemExit("model has no outputs")

    for vi in graph.input:
        if vi.name in renames:
            vi.name = renames[vi.name]
    for vi in graph.output:
        if vi.name in renames:
            vi.name = renames[vi.name]
    for node in graph.node:
        node.input[:] = [renames.get(x, x) for x in node.input]
        node.output[:] = [renames.get(x, x) for x in node.output]

    onnx.checker.check_model(model)
    onnx.save(model, args.output)
    print(
        f"Saved {args.output} (input={args.input_name}, "
        f"output={args.output_name})"
    )


if __name__ == "__main__":
    main()
