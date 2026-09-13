"""固定した小さい実例を実モデルで検証する。通常testからは起動しない。"""
import json
import argparse
import os
from pathlib import Path
import time
import run


def main():
    defaults, roles = run.load_settings()
    cases = json.loads(Path(__file__).with_name("eval_cases.json").read_text())
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=[case["id"] for case in cases])
    args = parser.parse_args()
    if args.case:
        cases = [case for case in cases if case["id"] == args.case]
    results = []
    lock = run.acquire_lock()
    try:
        for case in cases:
            settings = run.select_effort(defaults, case["role"])
            started = time.monotonic()
            path, start = case.get("file", case["id"]), case.get("start", 1)
            try:
                run.validate_question(case["role"], case["question"], case["source"], settings)
                with run.deadline(settings["timeout_seconds"]):
                    answer, usage = run.infer(case["role"], case["question"], case["source"], settings, roles, path, start)
            except (run.Fallback, OSError, ValueError, KeyError, TypeError) as error:
                reason = str(error) if isinstance(error, run.Fallback) else type(error).__name__
                result = {"case": case["id"], "passed": bool(case.get("expected_fallback")) and reason == case["expected_fallback"], "reason": reason}
                results.append(result)
                print(json.dumps(result), flush=True)
                continue
            if case.get("abstain"):
                passed = answer.get("abstain") is True and answer.get("line_ids") == []
                findings = []
            else:
                try:
                    findings = (run.validate_inspection(answer, case["source"], path, start, len(case["decisions"]))
                                if case["role"] == "inspect" else
                                run.validate_answer(answer, case["role"], case["source"], path, start))
                except run.Fallback as error:
                    result = {"case": case["id"], "passed": False, "reason": str(error), "raw_answer": answer}
                    print(json.dumps(result, ensure_ascii=False), flush=True)
                    results.append(result)
                    continue
                ids = {int(e["source"].rsplit(":", 1)[1]) for f in findings for e in f["evidence"]}
                text = " ".join(f["text"] for f in findings)
                if case["role"] == "inspect":
                    passed = not case.get("expected_fallback") and findings[0]["decisions"] == case["decisions"]
                elif case["role"] == "extract":
                    passed = ids == set(case["lines"])
                else:
                    passed = set(case["evidence"]).issubset(ids) and all(w in text for w in case["keywords"])
                    passed = passed and not any(w in text for w in case.get("forbidden", []))
            result = {"case": case["id"], "passed": passed, "findings": findings,
                      "elapsed_seconds": round(time.monotonic() - started, 2), "usage": usage}
            print(json.dumps(result, ensure_ascii=False), flush=True)
            results.append(result)
    finally:
        os.close(lock)
    return 0 if all(item["passed"] for item in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
