import math, json
SC=1000.0
# true populations (scaled fluorescence units)
P1=[(200+i*30,180+i*25,250+i*20) for i in range(8)]
P2=[(3000+i*200,500+i*50,600+i*40) for i in range(6)]
P3=[(400+i*50,600+i*60,4000+i*250) for i in range(5)]
true_vals=P1+P2+P3
S=[[1.00,0.10,0.02],[0.08,1.00,0.06],[0.03,0.05,1.00]]
def matvec(M,v): return [sum(M[i][j]*v[j] for j in range(3)) for i in range(3)]
def inv3(m):
    a,b,c,d,e,f,g,h,i=m
    A=e*i-f*h; B=-(d*i-f*g); C=d*h-e*g
    det=a*A+b*B+c*C
    return det, [[A, -(b*i-c*h), b*f-c*e],[B,a*i-c*g,-(a*f-c*d)],[C,-(a*h-b*g),a*e-b*d]]
det,cof=inv3(S)
Sinv=[[cof[r][k]/det for k in range(3)] for r in range(3)]
obs=[(idx+1,)+tuple(matvec(S,v)) for idx,v in enumerate(true_vals)]
comp=[(idx+1,)+tuple(matvec(Sinv,list(o[1:]))) for idx,o in enumerate(obs)]
M=150.0; A=0.5; den=math.asinh(A)
def tf(x): return math.asinh(x/M)/den
trans=[(idx+1,)+tuple(tf(v) for v in c[1:]) for idx,c in enumerate(comp)]
print("observed raw (float32-able):")
for r in obs: print(" ",[round(x,3) for x in r])
print("transformed:")
for r in trans: print(" ",[r[0]]+[round(x,4) for x in r[1:]])
# gates
def inside(x,y,poly):
    n=len(poly); inside=False;j=n-1
    for i in range(n):
        xi,yi=poly[i];xj,yj=poly[j]
        if ((yi>y)!=(yj>y)) and (x < (xj-xi)*(y-yi)/(yj-yi)+xi): inside=not inside
        j=i
    return inside
lymph=[(0.0,0.0),(0.9,0.0),(0.9,0.9),(0.0,0.9)]
fl1pos=[(0.9,0.0),(2.0,0.0),(2.0,2.0),(0.9,2.0)]
fl3pos=[(0.0,0.9),(2.0,0.9),(2.0,2.0),(0.0,2.0)]
pts={r[0]:(r[1],r[2],r[3]) for r in trans}
ly=[k for k,(a,b,c) in pts.items() if inside(a,b,lymph)]
f1=[k for k in ly if inside(pts[k][0],pts[k][1],fl1pos)]
f3=[k for k in ly if inside(pts[k][2],pts[k][1],fl3pos)]
print("lymph",sorted(ly),len(ly))
print("fl1pos(of lymph)",sorted(f1),len(f1))
print("fl3pos(of lymph)",sorted(f3),len(f3))
json.dump({"S":S,"Sinv":Sinv,"det":det,"obs":obs,"trans":trans,
  "gates":{"lymph":lymph,"fl1pos":fl1pos,"fl3pos":fl3pos},
  "members":{"lymph":sorted(ly),"fl1pos":sorted(f1),"fl3pos":sorted(f3)}},
  open("scripts/design.json","w"),indent=1)
