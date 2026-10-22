"""Independent Python reference for migration review. No APPDATA access or image writes."""
import argparse
from dataclasses import asdict
import json
from pathlib import Path
import sys
from unittest.mock import patch
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from mdript.catalog import Catalog
from mdript.pipeline import Cropper, Options, collect_images

def main():
    p=argparse.ArgumentParser();p.add_argument("images",type=Path);p.add_argument("report",type=Path);args=p.parse_args()
    args.report.parent.mkdir(parents=True,exist_ok=True)
    user_dir=args.report.parent / "empty-user-data"
    user_dir.mkdir(exist_ok=True)
    with patch("mdript.catalog.user_data_dir",return_value=str(user_dir)):
        catalog=Catalog()
    cropper=Cropper(catalog,Options(output_dir=str(args.report.parent / "unused-output"),write_csv=False,workers=1,skip_duplicate=False))
    rows=[]
    files=collect_images([str(args.images)],True)
    with patch.object(cropper,"_save",return_value=(None,"")):
        for i,path in enumerate(files,1):
            row=asdict(cropper.process_file(path));row.pop("saved_path",None);rows.append(row)
            if i%25==0 or i==1: print(f"Reference {i}/{len(files)}",flush=True)
    args.report.write_text(json.dumps({"count":len(rows),"hash_count":catalog.hash_count,"results":rows},ensure_ascii=False,indent=2),encoding="utf-8")
    print({s:sum(r["status"]==s for r in rows) for s in sorted(set(r["status"] for r in rows))})
if __name__=="__main__": main()
