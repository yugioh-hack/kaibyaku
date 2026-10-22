"""Independent end-to-end differential review against unchanged Python."""
import argparse
from dataclasses import asdict
import json
from pathlib import Path
import subprocess
import sys
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT));sys.path.insert(0,str(ROOT/"tests"))
import numpy as np
from PIL import Image
from test_basic import fake_card,fake_screenshot,draw_text_strokes
from mdript.layout import predict_card_box
from mdript.phash import artwork_phash_hex
from mdript.catalog import Catalog
from mdript.pipeline import Cropper,Options

def main():
    p=argparse.ArgumentParser();p.add_argument("directory",type=Path);p.add_argument("--exe",type=Path);args=p.parse_args()
    root=args.directory.resolve();root.mkdir(parents=True,exist_ok=True)
    inputs=root/"inputs";inputs.mkdir(exist_ok=True)
    user=root/"user";user.mkdir(exist_ok=True)
    hashes={"900001":[artwork_phash_hex(fake_card(seed=i)) for i in (0,1)],"900002":[artwork_phash_hex(fake_card(seed=2))]}
    names={"900001":{"n":"検証カード","e":"Review Card"},"900002":{"n":"CON","e":"CON"}}
    # Pillow BOX rounds each horizontal row before the vertical reduction.
    # This fixture makes the blue-red threshold exactly 40 only with that behavior.
    twin=fake_card(width=100,height=150,seed=8)
    w,h=twin.size;l,t,r,b=int(w*.04),int(h*.30),int(w*.08),int(h*.60)
    for y in range(t,b):
        for x in range(l,r):
            blue=140 if y-t < (b-t+1)//2 and (x-l)%2 else 139
            twin.putpixel((x,y),(100,100,blue))
    twin_hash=artwork_phash_hex(twin)
    hashes.update({"4370":[twin_hash],"19092":[twin_hash]})
    names.update({"4370":{"n":"境界カード"},"19092":{"n":"境界カード"}})
    twin.save(inputs/"10-frame-threshold.png")
    for offset,cid in enumerate(["5271","5272"]):
        special=fake_card(seed=7)
        w,h=special.size;sw,sh=int(w*.50)-int(w*.20),int(h*.50)-int(h*.40)
        pixels=np.random.default_rng(100+offset).integers(0,256,(sh,sw,3),dtype=np.uint8)
        special.paste(Image.fromarray(pixels),(int(w*.20),int(h*.40)))
        hashes[cid]=[artwork_phash_hex(special,with_center=True)]
        names[cid]={"n":"中央判定"+cid}
        special.save(inputs/f"{11+offset}-special-{cid}.png")
    (user/"card_hashes_user.json").write_text(json.dumps({"meta":{"format":2},"hashes":hashes}),encoding="utf-8")
    (user/"card_names.json").write_text(json.dumps({"cards":names}),encoding="utf-8")
    base=fake_screenshot();box=predict_card_box(base.size)
    ja=draw_text_strokes(base,box,vertical=False);en=draw_text_strokes(base,box,vertical=True)
    for name,im in [("01-ja.png",ja),("02-duplicate.png",ja),("03-en.png",en),("04-alt.png",fake_screenshot(seed=1)),("05-reserved.png",fake_screenshot(seed=2)),("06-crop.png",fake_card(seed=2)),("07-small.png",Image.new("RGB",(1,1))),("08-portrait.png",Image.new("RGB",(40,12000)))]: im.save(inputs/name)
    (inputs/"09-broken.png").write_bytes(b"not an image")
    cases=[("png",{}),("both",{"save_format":"both"}),("split",{"split":"both"}),("no-rounded",{"rounded":False}),("no-not-card",{"save_not_card":False})]
    output=[]
    for label,extra in cases:
        pyout=root/(label+"-python");rsout=root/(label+"-rust")
        if pyout.exists() or rsout.exists(): raise RuntimeError("use a fresh directory for each review run")
        with patch("mdript.catalog.user_data_dir",return_value=str(user)): catalog=Catalog()
        options=Options(output_dir=str(pyout),save_format="png",workers=1,write_csv=False,**{k:v for k,v in extra.items() if k!="save_format"})
        if "save_format" in extra: options.save_format=extra["save_format"]
        summary=Cropper(catalog,options).process([str(inputs)])
        expected=[asdict(r) for r in summary.results]
        record={"case":label,"python":expected}
        if args.exe:
            cmd=[str(args.exe.resolve()),"identify",str(inputs),"--output",str(rsout),"--user-data",str(user),"--format",options.save_format]
            if label=="split":cmd += ["--split","both"]
            if label=="no-rounded":cmd += ["--no-rounded"]
            if label=="no-not-card":cmd += ["--no-not-card"]
            proc=subprocess.run(cmd,capture_output=True,text=True,encoding="utf-8")
            record["exit_code"]=proc.returncode
            try: actual=json.loads(proc.stdout)["results"]
            except Exception: record["invocation_error"]={"stdout":proc.stdout,"stderr":proc.stderr};output.append(record);continue
            record["rust"]=actual;diffs=[]
            if len(actual)!=len(expected):diffs.append({"result_count":{"python":len(expected),"rust":len(actual)}})
            by_source={Path(x["source"]).name:x for x in actual}
            for ex in expected:
                name=Path(ex["source"]).name;got=by_source.get(name)
                if got is None:diffs.append({"source":name,"missing":True});continue
                fields=["status","card_id","name","distance","margin","english"]
                if ex["status"]!="エラー":fields.append("note")
                for key in fields:
                    if ex[key]!=got.get(key):diffs.append({"source":name,"field":key,"python":ex[key],"rust":got.get(key)})
                for side,row,dest in [("python",ex,pyout),("rust",got,rsout)]:
                    value=row.get("saved_path") or ""; row["relative_saved"]=str(Path(value.removeprefix(chr(92)*2+"?"+chr(92))).relative_to(dest)) if value else ""
                if ex["relative_saved"]!=got["relative_saved"]:diffs.append({"source":name,"field":"relative_saved","python":ex["relative_saved"],"rust":got["relative_saved"]})
            pyfiles={x.relative_to(pyout) for x in pyout.rglob("*") if x.is_file() and x.suffix in {".png",".webp"}}
            rsfiles={x.relative_to(rsout) for x in rsout.rglob("*") if x.is_file() and x.suffix in {".png",".webp"}}
            for path in pyfiles^rsfiles:diffs.append({"file":str(path),"file_set_mismatch":True})
            for path in pyfiles&rsfiles:
                with Image.open(pyout/path) as im:a=np.asarray(im.convert("RGBA"))
                with Image.open(rsout/path) as im:b=np.asarray(im.convert("RGBA"))
                if a.shape!=b.shape or not np.array_equal(a,b):diffs.append({"file":str(path),"pixels_equal":False,"python_shape":a.shape,"rust_shape":b.shape})
            record["differences"]=diffs
        output.append(record);print(label,"differences",len(record.get("differences",[])),flush=True)
    (root/"report.json").write_text(json.dumps(output,ensure_ascii=False,indent=2),encoding="utf-8")
    return 1 if any(r.get("differences") or "invocation_error" in r for r in output) else 0
if __name__=="__main__":raise SystemExit(main())
