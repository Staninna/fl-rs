import sys
a = [tuple(map(float, l.split())) for l in open(sys.argv[1])]
b = [tuple(map(float, l.split())) for l in open(sys.argv[2])]
tol = float(sys.argv[3]) if len(sys.argv) > 3 else 1e-6
n = min(len(a), len(b))
worst = 0
first = None
for i in range(n):
    for s in range(2):
        d = abs(a[i][s] - b[i][s])
        if d > tol and first is None:
            first = i
        worst = max(worst, d)
ma = max(abs(x) for p in a for x in p)
mb = max(abs(x) for p in b for x in p)
print('n', n, 'max|ref|', ma, 'max|port|', mb, 'worst diff', worst, 'first >tol at', first)
if first is not None:
    for i in range(max(0, first - 2), min(n, first + 5)):
        print(i, a[i], b[i])
