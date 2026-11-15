#!/bin/sh
# Render README screenshots of the real UI with demo data (docs/screenshots/demo.js),
# using headless Chrome in dark mode. Usage: make screenshots
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/docs/screenshots"
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
[ -x "$CHROME" ] || { echo "needs Google Chrome (set CHROME=/path/to/chrome)" >&2; exit 1; }

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp "$ROOT"/app/ui/* "$OUT/demo.js" "$STAGE/"

WALL='background:linear-gradient(135deg,#1c2541 0%,#3a506b 55%,#5bc0be 130%)'

# page <name> <html> <after-load script> [body style]
page() {
  sed -e "s#<script src=\"common.js\"></script>#<script src=\"demo.js\"></script><script src=\"common.js\"></script>#" \
      -e "s#</body>#<script>$3</script></body>#" \
      ${4:+-e "s|<body\\([^>]*\\)>|<body\\1 style=\"$4\">|"} \
      "$STAGE/$2" > "$STAGE/shot-$1.html"
}

shot() { # shot <name> <width> <height>
  "$CHROME" --headless=new --disable-gpu --hide-scrollbars --allow-file-access-from-files \
    --blink-settings=preferredColorScheme=0 --force-device-scale-factor=2 \
    --window-size="$2,$3" --virtual-time-budget=2500 \
    --screenshot="$STAGE/$1.png" "file://$STAGE/shot-$1.html" >/dev/null 2>&1
  sips -Z 1600 "$STAGE/$1.png" --out "$OUT/$1.png" >/dev/null
  echo "  $OUT/$1.png"
}

# GPU sparklines need history; seed a plausible series per GPU, then show a page.
SEED='setTimeout(()=>{S.mhist=DEMO.mhist;for(const t of DEMO.snapshot.targets)for(const g of t.gpus){let v=g.util,h=[];for(let i=0;i<90;i++){v=Math.max(0,Math.min(100,v+(Math.random()-.5)*14+(g.util-v)*.15));h.push(v)}S.hist.set(`${t.name}/${g.job_id??""}/${g.index}`,h)}'

page home index.html "$SEED;showPage('home')},200)"
page agents index.html "$SEED;S.agentOpen.add('claude_code::a2');showPage('agents')},200)"
page compute index.html "$SEED;S.view='all';showPage('compute')},200)"
page panel panel.html "" "$WALL;padding:36px 60px;height:auto;overflow:visible"
sed -i '' 's#</head>#<style>.panel{width:380px!important;height:auto!important}.p-scroll{overflow:visible!important}</style></head>#' "$STAGE/shot-panel.html"
page fox float.html "message({kind:'done',title:'train-vit finished',detail:'hpc · job 7731_3 · COMPLETED'})" "$WALL;padding:24px 28px"
sed -i '' 's#</head>#<style>\#stage{position:relative;width:340px;height:150px}.bubble{animation:none!important}.fb .buddy{animation:none!important}</style></head>#' "$STAGE/shot-fox.html"

echo "Rendering screenshots:"
shot home 1240 1040
shot agents 1240 780
shot compute 1240 780
shot panel 500 880
shot fox 400 200
