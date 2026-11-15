# ISIS icons

Role icons from EVE Online's ISIS ship tree. They are CCP's art, taken from
the EVE University wiki: <https://wiki.eveuniversity.org/UniWiki:Icons>.
EVE Online and all related art are trademarks and property of CCP hf.

The CIC module (`src/modules/cic/icons.rs`) draws them in its own colours,
so only their transparency is kept: each `.a8` file is 30 × 30 bytes, one
alpha value (0 to 255) per pixel, row by row from the top left.

| File        | Wiki file                               | Used for           |
|-------------|-----------------------------------------|--------------------|
| shield.a8   | Icon_ISIS_defense_Shields.png           | Shield reps        |
| armor.a8    | Icon_ISIS_defense_armor.png             | Armor reps         |
| cap.a8      | Icon_ISIS_Energydestabilization.png     | Capacitor          |
| attack.a8   | Icon_ISIS_attack.png                    | Damage, DMG badge  |
| ecm.a8      | Icon_ISIS_Ecm.png                       | ECM badge          |
| warp.a8     | Icon_ISIS_Warpprevention.png            | POINT/SCRAM badges |

To make one from a PNG (Python with Pillow):

```python
from PIL import Image
image = Image.open("Icon_ISIS_Ecm.png").convert("RGBA")
open("ecm.a8", "wb").write(bytes(pixel[3] for pixel in image.getdata()))
```
