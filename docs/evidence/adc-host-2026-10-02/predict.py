import re
import sys
from pathlib import Path
MASK=(1<<64)-1
for chip,adc,ref,mv,index in [('esp32s3','adc1',[3200,2400,1700,900],850,0),('esp32s3','adc2',[3240,2410,1720,915],850,0),('esp32c3','adc1',[2000]*4,1370,0),('esp32c6','adc1',[2850,2850,2900,2850],2800,4)]:
 t=(Path(sys.argv[1]) / f'{chip}-curve.txt').read_text()
 rows=[re.findall(r'\{(\d+),\s*(\d+(?:e\d+)?)\}',line) for line in t.split(f'{adc}_error_coef_atten',1)[1].split('};',1)[0].splitlines() if '//atten' in line or '//ADC' in line]
 signs=[list(map(int,re.findall(r'-?\d+',line.split('}')[0]))) for line in t.split(f'{adc}_error_sign',1)[1].split('};',1)[0].splitlines() if '//atten' in line or '//ADC' in line]
 coeff=[(int(a),int(float(b)),c) for (a,b),c in zip(rows[index+3],signs[index+3]) if int(float(b))]
 def convert(raw, scale):
  v=raw*(scale*mv//ref[3])//scale
  if v==0:return 0
  return v-sum(((v**i&MASK)*a&MASK)//b*c for i,(a,b,c) in enumerate(coeff))
 scale=1000000 if adc=='adc1' and chip=='esp32s3' else 65536
 raws=[min(range(4096),key=lambda r:abs(convert(r,scale)-v)) for v in [500,1000]]
 print(chip,adc,[(r,convert(r,65536)) for r in [raws[0],1024,raws[1],3072]])
