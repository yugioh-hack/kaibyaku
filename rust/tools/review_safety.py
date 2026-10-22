"""CLI rejection, output exclusion and cross-process save collision checks."""
from concurrent.futures import ThreadPoolExecutor
import argparse,json,subprocess
from pathlib import Path
from PIL import Image

def main():
    p=argparse.ArgumentParser();p.add_argument("fixtures",type=Path);p.add_argument("directory",type=Path);p.add_argument("--exe",type=Path,required=True);args=p.parse_args()
    root=args.directory.resolve();root.mkdir(parents=True,exist_ok=False)
    fixtures=args.fixtures.resolve();exe=str(args.exe.resolve());source=fixtures/"inputs/01-ja.png";user=fixtures/"user"
    def call(extra,source=source,out=None):
        return subprocess.run([exe,"identify",str(source),"--output",str(out or root/"out"),"--user-data",str(user),*extra],capture_output=True,text=True,encoding="utf-8",timeout=120)
    checks=[]
    for flags in [["--quality","0"],["--quality","nan"],["--format","junk"],["--resolution","0x1080"],["--unknown"],["--box","1"]]:
        r=call(flags);checks.append({"case":"reject "+" ".join(flags),"pass":r.returncode!=0})
    with ThreadPoolExecutor(max_workers=4) as pool:results=list(pool.map(lambda _:call(["--format","both","--keep-duplicates"]),range(4)))
    files=list((root/"out").glob("*"))
    valid=len(files)==8 and all(r.returncode==0 for r in results)
    for file in files:
        try:
            with Image.open(file) as im:im.load()
        except Exception:valid=False
    checks.append({"case":"four processes reserve distinct both-format outputs","pass":valid,"files":len(files)})
    out=root/"exclude";out.mkdir();copied=out/"input.png";copied.write_bytes(source.read_bytes())
    r=call(["--dry-run"],source=copied,out=out)
    checks.append({"case":"explicit input inside output excluded","pass":r.returncode==0 and json.loads(r.stdout).get("results")==[]})
    (root/"report.json").write_text(json.dumps(checks,indent=2),encoding="utf-8")
    print(json.dumps(checks,indent=2));return 0 if all(x["pass"] for x in checks) else 1
if __name__=="__main__":raise SystemExit(main())
