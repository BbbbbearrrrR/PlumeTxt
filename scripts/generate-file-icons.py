import csv
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont
rows = [
('MD','Markdown','md markdown','#3fddcf'),('TXT','Text','txt log text','#9aaeb8'),('PDF','PDF','pdf','#f2727a'),
('PNG','PNG image','png apng','#65d6aa'),('JPG','JPEG image','jpg jpeg jpe jfif','#94d580'),('GIF','GIF image','gif','#c08aea'),('BMP','Bitmap image','bmp dib','#74b8de'),('TIF','TIFF image','tif tiff','#61b9b2'),('ICO','Icon','ico','#b3bfff'),('JXR','JPEG XR image','jxr wdp hdp','#7cd1c6'),('WEBP','WebP image','webp','#5cc7e2'),('HEIC','HEIF image','heic heif','#bd9de3'),('AVIF','AVIF image','avif','#edb473'),
('RS','Rust source','rs','#e7a175'),('JS','JavaScript','js mjs cjs jsx','#edcf70'),('TS','TypeScript','ts tsx','#79b9f5'),('JSON','JSON data','json jsonc','#e2c471'),('TOML','TOML config','toml','#c69aea'),('YAML','YAML config','yaml yml','#db98b5'),('HTML','HTML source','html htm','#eb9970'),('CSS','CSS source','css scss less','#8fb1f4'),('PY','Python source','py pyw','#79bdcf'),('C','C source','c h','#83b8e7'),('CPP','C++ source','cpp cc cxx hpp hxx','#a59bea'),('CS','C# source','cs','#be92e3'),('JAVA','Java source','java','#e4a879'),('GO','Go source','go','#6dcde3'),('SH','Shell script','sh bash zsh','#98cda5'),('PS1','PowerShell script','ps1 psm1 psd1','#87aef4'),('XML','XML data','xml svg','#e2aa76'),('CSV','CSV data','csv tsv','#80cba7'),('INI','Configuration','ini cfg conf','#9bafbb')]
root=Path('assets/file-icons')
root.mkdir(parents=True, exist_ok=True)
# Keep existing resource IDs stable; give formerly grouped formats their own badge.
extra=[]
for index,(label,name,extensions,color) in enumerate(rows):
    primary,*aliases=extensions.split()
    rows[index]=(label,name,primary,color)
    for extension in aliases:
        if extension == 'markdown':
            rows[index]=(label,name,'md markdown mdown','#3fddcf')
        else:
            extra.append((extension.upper(),name,extension,color))
extra += [
('SQL','SQL source','sql','#71cbb5'),('VUE','Vue component','vue','#70d0ad'),
('SVLT','Svelte component','svelte','#ed986d'),('PHP','PHP source','php','#b3a1e8'),
('RB','Ruby source','rb','#ed8199'),('LUA','Lua source','lua','#87a9ee'),
('KT','Kotlin source','kt','#c09ae9'),('KTS','Kotlin script','kts','#bb92db'),
('SWIFT','Swift source','swift','#eca074'),('R','R source','r','#88b9dc'),
('DART','Dart source','dart','#74cde0'),('BAT','Batch script','bat','#a6c996'),
('CMD','Command script','cmd','#93b589'),('ENV','Environment config','env','#d3c18b'),
('LOCK','Lock file','lock','#bba8d9'),('PROP','Properties config','properties','#99bcc2'),
('TEX','LaTeX source','tex','#8acdb3'),('BIB','Bibliography','bib','#add099'),
('RST','reStructuredText','rst','#a3bccc'),('ADOC','AsciiDoc','adoc asciidoc','#80c7d1'),
('DIFF','Diff','diff','#d0bf78'),('PATCH','Patch','patch','#cfab80'),
('NB','Notebook JSON','ipynb','#dda377'),('GIT','Git ignore rules','gitignore','#df987b'),
('GATTR','Git attributes','gitattributes','#c59e86'),
('ECFG','Editor config','editorconfig','#93b7c0'),('GRDL','Gradle script','gradle','#8ac5b9')]
rows += extra
assert len({label.lower() for label,*_ in rows}) == len(rows)
extensions=[ext for _,_,exts,_ in rows for ext in exts.split()]
assert len(extensions) == len(set(extensions))
with Image.open('assets/feather.png') as source:
    feather=source.convert('RGBA')
    feather=feather.crop(feather.getchannel('A').getbbox())
with Path('assets/file-types.tsv').open('w',newline='',encoding='utf-8') as f:
    writer=csv.writer(f,delimiter='\t');writer.writerow(['id','label','name','extensions'])
    for i,(label,name,ext,color) in enumerate(rows,101):
        writer.writerow([i,label,name,ext])
        images=[]
        for size in [16,20,24,32,48,64,128,256]:
            scale=4;s=size*scale
            image=Image.new('RGBA',(s,s));d=ImageDraw.Draw(image)
            d.rounded_rectangle((s*.13,s*.035,s*.88,s*.96),radius=s*.09,fill='#101c23',outline=color,width=max(2,round(s*.04)))
            # Reuse the original brand asset; never redraw a second feather.
            mark=feather.copy()
            mark.thumbnail((round(s*.62),round(s*.55)),Image.Resampling.LANCZOS)
            image.alpha_composite(mark,((s-mark.width)//2,round(s*.05)))
            d.rounded_rectangle((s*.02,s*.61,s*.98,s*.94),radius=s*.045,fill=color)
            fs=int(s*(.28 if len(label)<=3 else .22 if len(label)==4 else .18));font=ImageFont.truetype('C:/Windows/Fonts/arialbd.ttf',fs)
            box=d.textbbox((0,0),label,font=font);w=box[2]-box[0];h=box[3]-box[1]
            d.text(((s-w)/2-box[0],s*.775-h/2-box[1]),label,font=font,fill='#0b1820')
            images.append(image.resize((size,size),Image.Resampling.LANCZOS))
        images[-1].save(root/f'{label.lower()}.ico',format='ICO',sizes=[im.size for im in images],append_images=images[:-1])
        with Image.open(root/f'{label.lower()}.ico') as check:
            assert check.ico.sizes()=={(n,n) for n in (16,20,24,32,48,64,128,256)}
# Preview is generated from the same icon sources.
board=Image.new('RGB',(800,((len(rows)+9)//10)*80),'#090d0f');d=ImageDraw.Draw(board)
for i,(label,*_) in enumerate(rows):
    with Image.open(root/f'{label.lower()}.ico') as ico:
        icon=ico.ico.getimage((48,48));board.paste(icon,((i%10)*80+16,(i//10)*80+4),icon)
    d.text(((i%10)*80+15,(i//10)*80+56),label,fill='#dee8e9')
board.save(root/'preview.png')
print(f'Generated and verified {len(rows)} icons for {len(extensions)} extensions.')
