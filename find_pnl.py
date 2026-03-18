import os
import re
results = []
for root, dirs, files in os.walk('d:/Deriv'):
    if 'target' in root: continue
    for file in files:
        if file.endswith('.txt') or file.endswith('.log'):
            path = os.path.join(root, file)
            try:
                with open(path, 'r', encoding='utf-8', errors='ignore') as f:
                    content = f.read()
                    pnls = re.findall(r'pnl="([-0-9.]+)"', content)
                    if not pnls: pnls = re.findall(r'real_pnl=([-0-9.]+)', content)
                    if pnls:
                        nums = [float(x) for x in pnls]
                        if min(nums) < -1500:
                            results.append(f'{path}: min {min(nums)}, max {max(nums)}')
            except: pass
print('\n'.join(results) if results else 'NO LOGS FOUND WITH PNL < -1500')
