"""Copy a private Swiss MATSim timetable/config locally and point routing input at that copy."""
import argparse, shutil, xml.etree.ElementTree as ET
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument("--schedule",type=Path,required=True);p.add_argument("--config",type=Path,required=True)
p.add_argument("--output",type=Path,default=Path(__file__).parent/"local")
a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True)
shutil.copy2(a.schedule,a.output/"schedule.xml.gz");shutil.copy2(a.config,a.output/"original_config.xml")
tree=ET.parse(a.config)
for module in tree.getroot().findall("module"):
    if module.get("name")=="transit":
        for param in module.findall("param"):
            if param.get("name")=="transitScheduleFile":param.set("value","schedule.xml.gz")
content=ET.tostring(tree.getroot(),encoding="unicode")
(a.output/"config.xml").write_text('<?xml version="1.0" encoding="utf-8"?>\n<!DOCTYPE config SYSTEM "http://www.matsim.org/files/dtd/config_v2.dtd">\n'+content,encoding="utf-8")
print(a.output.resolve())
