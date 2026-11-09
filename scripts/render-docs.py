"""Generate documentation media from Muisland's offline sample renders.

Usage: python scripts/render-docs.py <preview-folder> <docs/images-folder>
Requires Pillow. Run Muisland.exe --render-previews <preview-folder> first.
"""
import math,sys
from pathlib import Path
from PIL import Image,ImageDraw,ImageFont,ImageFilter

source=Path(sys.argv[1]);out=Path(sys.argv[2]);out.mkdir(parents=True,exist_ok=True)
accent='#c3b2ff';muted='#aaa0ba';white='#f2eefb';bg='#17121f'
fontdir=Path('C:/Windows/Fonts')
def font(size,bold=False):
    try:return ImageFont.truetype(str(fontdir/('seguisb.ttf' if bold else 'segoeui.ttf')),size)
    except OSError:return ImageFont.load_default(size=size)
def label(draw,xy,value,size=18,color=white,bold=False):draw.text(xy,value,font=font(size,bold),fill=color)
def native(name,kind='menu'):
    img=Image.open(source/(name+'.bmp')).convert('RGB');w,h=img.size
    mask=Image.new('L',(w*4,h*4));d=ImageDraw.Draw(mask)
    if kind=='circle':d.ellipse((0,0,w*4-1,h*4-1),fill=255)
    elif kind=='hover':
        d.ellipse((0,56*4,76*4-1,132*4-1),fill=255)
        d.rounded_rectangle((94*4,0,w*4-1,h*4-1),radius=22*4,fill=255)
        d.polygon([(84*4,94*4),(97*4,81*4),(97*4,107*4)],fill=255)
    else:d.rounded_rectangle((0,0,w*4-1,h*4-1),radius=34*4,fill=255)
    mask=mask.resize((w,h),Image.Resampling.LANCZOS)
    pixels=[]
    for rgb,a in zip(img.getdata(),mask.getdata()):
        alpha=a/255
        pixels.append(tuple(max(0,min(255,round((c-42*(1-alpha))/alpha))) for c in rgb)+(a,) if a else (0,0,0,0))
    rgba=Image.new('RGBA',(w,h));rgba.putdata(pixels);return rgba
def board(w=1200,h=760):
    image=Image.new('RGBA',(w,h),bg);d=ImageDraw.Draw(image)
    for x in range(20,w,28):
        for y in range(20,h,28):d.point((x,y),fill='#302438')
    d.rounded_rectangle((24,24,w-25,h-25),radius=30,outline='#44324e',width=1)
    return image
def paste(image,item,xy):
    shadow=Image.new('RGBA',image.size);shadow.paste((0,0,0,105),(xy[0]+8,xy[1]+18,xy[0]+item.width+8,xy[1]+item.height+18));shadow=shadow.filter(ImageFilter.GaussianBlur(26));image.alpha_composite(shadow);image.alpha_composite(item,xy)
accounts=native('accounts');prefs=native('preferences');circle=native('circle','circle');hover=native('circle-hover','hover');card=hover.crop((84,0,390,188))
for filename,title,subtitle,item,notes in [
 ('accounts.png','All your accounts.','A seamless switch, in one place.',accounts,['Named accounts','5-hour + weekly limits','Explicit switch confirmation']),
 ('preferences.png','Make it yours.','Small circle. Readable bubble.',prefs,['Independent menu size','Independent floating size','Independent hover bubble size'])]:
    image=board();d=ImageDraw.Draw(image);label(d,(70,80),'MUISLAND / WINDOWS',14,accent,True);label(d,(70,160),title,44,white,True);label(d,(70,225),subtitle,19,muted)
    for j,n in enumerate(notes):label(d,(74,335+j*53),'0'+str(j+1),13,accent,True);label(d,(112,330+j*53),n,18)
    label(d,(70,664),'NATIVE APP · SAMPLE DATA',12,muted);paste(image,item,(690,112));image.convert('RGB').save(out/filename,optimize=True)
image=board(1200,550);d=ImageDraw.Draw(image);label(d,(70,70),'A CIRCLE WITH ROOM TO BREATHE.',30,white,True);label(d,(70,122),'Hover for detail. Keep your desktop clear.',19,muted)
paste(image,hover.resize((585,282),Image.Resampling.LANCZOS),(308,202));label(d,(70,485),'Separate bubble · Automatic side selection · Independent sizes',15,accent);image.convert('RGB').save(out/'circle-bubble.png',optimize=True)
# Illustrated motion uses native sample pixels and the app's easing vocabulary.
frames=[];durations=[]
def motion_frame(t):
    image=board(1000,700);d=ImageDraw.Draw(image);label(d,(54,48),'MUISLAND / IN MOTION',13,accent,True)
    label(d,(54,77),'One little island. Everything within reach.',26,white,True)
    phase='Hover the circle'
    if t<.6:
        image.alpha_composite(circle,(365,300))
    elif t<2.2:
        p=min(1,(t-.6)/.32);ease=1-(1-p)**3;zoom=.94+.06*ease
        image.alpha_composite(circle,(365,300));c=card.resize((round(card.width*zoom),round(card.height*zoom)),Image.Resampling.LANCZOS);c.putalpha(c.getchannel('A').point(lambda a:round(a*ease)));image.alpha_composite(c,(449+round(8*(1-ease)),244+round((188-c.height)/2)))
        phase='A separate usage bubble'
    elif t<4.4:
        p=min(1,(t-2.2)/.36);ease=1-(1-p)**3
        w=round(76+(420-76)*ease);h=round(76+(536-76)*ease)
        item=accounts.resize((w,h),Image.Resampling.LANCZOS);image.alpha_composite(item,(round(403-w/2),round(300+(142-300)*ease)))
        phase='Click to open your accounts'
    else:
        p=min(1,(t-4.4)/.36);ease=1-(1-p)**3;w=round(420+(76-420)*ease);h=round(536+(76-536)*ease)
        if p<1:image.alpha_composite(accounts.resize((w,h),Image.Resampling.LANCZOS),(round(403-w/2),round(142+(300-142)*ease)))
        else:image.alpha_composite(circle,(365,300))
        phase='Close. Back to the same spot.'
    label(d,(54,638),phase,16,white,True);label(d,(670,642),'ILLUSTRATED · SAMPLE DATA',10,muted)
    return image.convert('RGB')
for i in range(132):frames.append(motion_frame(i/24));durations.append(40)
palette=frames[35].quantize(colors=192)
frames=[f.quantize(palette=palette,dither=Image.Dither.NONE) for f in frames]
frames[0].save(out/'interaction.gif',save_all=True,append_images=frames[1:],duration=durations,loop=0,optimize=True,disposal=2)
print('Generated',', '.join(p.name for p in out.iterdir()))
