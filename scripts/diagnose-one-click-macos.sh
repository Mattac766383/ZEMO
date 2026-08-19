#!/bin/zsh
set -u

print "ZEMO macOS one-click diagnostic"
print "macOS: $(sw_vers -productVersion 2>/dev/null || true)"
print "user: $(id -un)"
print

folders=("$HOME/Desktop" "$HOME/Documents" "$HOME/Downloads" "$HOME/Pictures" "$HOME/Movies")
labels=("Bureau" "Documents" "Téléchargements" "Images" "Vidéos")

for i in {1..5}; do
  p="${folders[$i]}"
  label="${labels[$i]}"
  print "=== $label ==="
  print "path=$p"
  if [[ ! -e "$p" ]]; then
    print "exists=NO"
    print
    continue
  fi
  ls -ldO@ "$p" 2>&1 | sed 's/^/root: /'
  count=0
  errors=0
  while IFS= read -r -d '' f; do
    (( count += 1 ))
    if ! stat -f '%N|type=%HT|size=%z|flags=%Sf' "$f" >/tmp/zemo-stat.$$ 2>/tmp/zemo-stat-err.$$; then
      (( errors += 1 ))
      print "STAT_ERROR: $f :: $(cat /tmp/zemo-stat-err.$$)"
    fi
    if (( count >= 25 )); then
      break
    fi
  done < <(find "$p" -mindepth 1 -maxdepth 1 -print0 2>/tmp/zemo-find-err.$$)
  find_err="$(cat /tmp/zemo-find-err.$$ 2>/dev/null || true)"
  [[ -n "$find_err" ]] && print "ENUM_ERROR: $find_err"
  print "sample_entries=$count stat_errors=$errors"
  print
 done

rm -f /tmp/zemo-stat.$$ /tmp/zemo-stat-err.$$ /tmp/zemo-find-err.$$ 2>/dev/null || true
print "END_ZEMO_DIAG"
