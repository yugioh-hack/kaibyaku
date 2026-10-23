"""Compare independently collected Python Result values with Rust identify."""
import argparse,json,subprocess
from pathlib import Path

def main():
    p=argparse.ArgumentParser();p.add_argument("reference",type=Path);p.add_argument("--exe",type=Path,required=True);p.add_argument("--report",type=Path,required=True);args=p.parse_args()
    ref=json.loads(args.reference.read_text(encoding="utf-8"));rows=ref["results"]
    if not rows: p.error("empty reference")
    roots={str(Path(r["source"]).parent) for r in rows}
    if len(roots)!=1:p.error("reference must contain a flat source directory")
    out=args.report.resolve().parent;out.mkdir(parents=True,exist_ok=True)
    cmd=[str(args.exe.resolve()),"identify",next(iter(roots)),"--output",str(out/"unused-rust-output"),"--user-data",str(out/"empty-user-data"),"--dry-run","--keep-duplicates"]
    run=subprocess.run(cmd,capture_output=True,text=True,encoding="utf-8",timeout=600)
    if not run.stdout:raise RuntimeError(run.stderr)
    result=json.loads(run.stdout);actual=result["results"]
    by_source={str(Path(r["source"]).resolve()).casefold():r for r in actual}
    differences=[]
    fields=["status","card_id","name","distance","margin","english","note"]
    matched=0
    for ex in rows:
        got=by_source.get(str(Path(ex["source"]).resolve()).casefold())
        if got is None:differences.append({"file":Path(ex["source"]).name,"missing":True});continue
        changes={key:{"python":ex[key],"rust":got.get(key)} for key in fields if ex[key]!=got.get(key)}
        if changes:differences.append({"file":Path(ex["source"]).name,"fields":changes})
        else:matched+=1
    report={"reference_count":len(rows),"rust_count":len(actual),"exact_result_matches":matched,"exit_code":run.returncode,"differences":differences,"rust_results":actual}
    args.report.write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding="utf-8")
    print(json.dumps({k:v for k,v in report.items() if k!="rust_results"},ensure_ascii=False,indent=2))
    return 0 if matched==len(rows)==len(actual) and run.returncode==0 else 1
if __name__=="__main__":raise SystemExit(main())
