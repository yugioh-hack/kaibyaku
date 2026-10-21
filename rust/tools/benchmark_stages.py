"""Repeatable in-memory stage benchmark, not an end-to-end application benchmark."""
import argparse, hashlib, io, json, platform, statistics, subprocess, sys, time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT))
import numpy as np
import PIL
from PIL import Image,features
from mdript.layout import predict_card_box
from mdript.phash import artwork_phash_hex
from mdript.pipeline import rounded_alpha

def measure(fn):
    fn()
    times=[]
    for _ in range(5):
        start=time.perf_counter(); value=fn(); times.append((time.perf_counter()-start)*1000)
    return times,value

def main():
    parser=argparse.ArgumentParser();parser.add_argument("images",type=Path);parser.add_argument("--output",type=Path,required=True);args=parser.parse_args()
    out=args.output.resolve();out.mkdir(parents=True,exist_ok=True)
    files=sorted(p for p in args.images.rglob("*") if p.suffix.lower() in {".png",".jpg",".jpeg",".webp"})
    if not files: parser.error("no images")
    # Uniform deterministic coverage of filenames, without choosing by performance.
    chosen=[files[i] for i in sorted(set(np.linspace(0,len(files)-1,min(12,len(files)),dtype=int)))]
    manifest=[];rows=[]
    for i,path in enumerate(chosen):
        data=path.read_bytes()
        def decode(data):
            with Image.open(io.BytesIO(data)) as im: return im.convert("RGB")
        rgb=decode(data);box=predict_card_box(rgb.size)
        if not (0<=box[0]<box[2]<=rgb.width and 0<=box[1]<box[3]<=rgb.height): raise ValueError("invalid predicted box")
        png=io.BytesIO();rgb.save(png,"PNG");png=png.getvalue()
        rgba=rounded_alpha(rgb.crop(box))
        png_path=out/f"{i}-screen.png";png_path.write_bytes(png)
        rgba_path=out/f"{i}-rgba.png";rgba.save(rgba_path)
        manifest.append({"id":i,"source":str(path.resolve()),"png":str(png_path),"rgba":str(rgba_path),"rust_output":str(out/f"{i}-rust.webp")})
        def encode():
            buf=io.BytesIO();rgba.save(buf,"WEBP",quality=90,method=4);return buf.getvalue()
        row={"id":i,"input_sha256":hashlib.sha256(data).hexdigest(),"size":rgb.size,"format":path.suffix}
        row["decode_rgb"],_=measure(lambda:decode(data))
        row["png_decode_rgb"],_=measure(lambda:decode(png))
        row["phash_128"],row["hash"]=measure(lambda:artwork_phash_hex(rgb.crop(box)))
        row["webp_encode"],encoded=measure(encode)
        (out/f"{i}-python.webp").write_bytes(encoded);row["bytes"]=len(encoded);rows.append(row)
        print(f"Python {i+1}/{len(chosen)}",file=sys.stderr,flush=True)
    manifest_path=out/"manifest.json";manifest_path.write_text(json.dumps(manifest),encoding="utf-8")
    proc=subprocess.run([str(ROOT/"rust/target/release/stage-bench.exe"),str(manifest_path)],capture_output=True,text=True,encoding="utf-8",check=True)
    rust=json.loads(proc.stdout)
    stages=["decode_rgb","png_decode_rgb","phash_128","webp_encode"]
    summary={}
    for stage in stages:
        py=sum(statistics.median(row[stage]) for row in rows)
        rs=sum(statistics.median(row[stage]) for row in rust)
        summary[stage]={"python_ms":py,"rust_ms":rs,"speedup":py/rs}
    checks=[]
    for i,(py,rs) in enumerate(zip(rows,rust)):
        a=(out/f"{i}-python.webp").read_bytes();b=(out/f"{i}-rust.webp").read_bytes()
        with Image.open(io.BytesIO(a)) as im: ap=np.asarray(im.convert("RGBA")).astype(np.int16)
        with Image.open(io.BytesIO(b)) as im: bp=np.asarray(im.convert("RGBA")).astype(np.int16)
        checks.append({"id":i,"hash_equal":py["hash"]==rs["hash"],"webp_bytes_equal":a==b,"decoded_equal":bool(np.array_equal(ap,bp)),"max_channel_difference":int(np.abs(ap-bp).max())})
    report={"method":"12 evenly spaced real inputs; warmup then 5 repeats, sum of per-image medians; single-thread, in-memory; Python runs before Rust; not full pipeline","environment":{"python":platform.python_version(),"pillow":PIL.__version__,"numpy":np.__version__,"pillow_webp":features.version("webp"),"platform":platform.platform()},"summary":summary,"checks":checks,"python":rows,"rust":rust}
    (out/"report.json").write_text(json.dumps(report,indent=2),encoding="utf-8")
    print(json.dumps({"summary":summary,"checks":checks},indent=2))
if __name__=="__main__":main()
