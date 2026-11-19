import random, json
random.seed(7)
words = ["alpha","beta","gamma","delta","epsilon","zeta","eta","theta"]
out = []
out.append("type Pt={x:int64,y:int64}")
import sys
N = int(sys.argv[1]) if len(sys.argv) > 1 else 25000
for i in range(N):
    r = random.random()
    if i % 97 == 0:
        out.append(json.dumps(random.choice(words)))  # dynamic root: string
        continue
    if i % 199 == 0:
        out.append(str(random.randint(-10**12, 10**12)))  # dynamic root: int
        continue
    f = []
    f.append(f"seq:{i}")
    f.append(f"rnd:{random.randint(-2**62, 2**62)}")
    f.append(f"k1000:{random.randint(0,10**6)*1000}")
    f.append(f"small:{random.randint(0,5)}::uint8")
    f.append(f"u:{random.randint(0,2**64-1)}::uint64")
    f.append(f"i32:{random.randint(-2**31,2**31-1)}::int32")
    f.append(f"w:{json.dumps(random.choice(words))}")
    f.append(f"s:{json.dumps('str'+str(random.randint(0,100000)))}")
    f.append(f"c:\"const\"")
    f.append(f"fl:{random.random()*1000:.6f}")
    f.append(f"f32:{random.uniform(-10,10):.3f}::float32")
    f.append(f"b:{'true' if random.random()<0.5 else 'false'}")
    f.append(f"ts:{1700000000000000000 + i*1000000}::timestamp" if False else f"d:{random.randint(0,10**9)}ns")
    if random.random() < 0.3:
        f.append(f"un:{random.randint(0,9)}::(int64|string)")
    else:
        f.append(f"un:{json.dumps(random.choice(words))}::(int64|string)")
    if random.random() < 0.5:
        f.append(f"opt?:{random.randint(0,100)}")
    else:
        f.append(f"opt?:none::int64")
    n = random.randint(0,4)
    f.append("arr:[" + ",".join(str(random.randint(-5,5)) for _ in range(n)) + "]::[int64]")
    f.append("m:map{" + ",".join(f"{json.dumps(random.choice(words))}:{random.randint(0,9)}" for _ in range(random.randint(0,3))) + "}::map{string:int64}")
    f.append("st:set[" + ",".join(str(random.randint(0,20)) for _ in range(random.randint(0,3))) + "]::set[int64]")
    f.append(f"pt:{{x:{i},y:{-i}}}::Pt")
    f.append(f"ip:10.{i%256}.{(i//256)%256}.1")
    f.append(f"en:{json.dumps(random.choice(['a','b','c']))}::enum(a,b,c)")
    if random.random() < 0.1:
        f.append("er:error(\"bad\")")
    else:
        f.append("er:null")
    f.append("nested:{a:[{b:" + str(i%7) + "}],m:" + ("null" if i%3 else "{z:1}") + "}")
    out.append("{" + ",".join(f) + "}")
print("\n".join(out))
