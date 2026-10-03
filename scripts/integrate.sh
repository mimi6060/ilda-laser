#!/usr/bin/env bash
# Integrate one finished task: GitHub PR -> review on the PR -> local merge
# into develop -> full suite (cargo test, clippy, e2e) -> push develop, which
# marks the PR merged. Red suite = no merge, the PR gets "CHANGES REQUESTED".
#
#   scripts/integrate.sh <branch> <task-id> <review-file>
#
# <review-file>: the reviewer's text (Markdown), ending in a "Verdict:" line.
# Preview only: the tests never use --device and never arm a laser.
set -euo pipefail
branch=$1 task=$2 review=$3
name=${branch#*/}
note=docs/prs/$name.md
root=$(git rev-parse --show-toplevel)
cd "$root"
export PATH=/opt/homebrew/opt/rustup/bin:$PATH

git checkout -q develop
git pull -q --ff-only origin develop
git push -q -u origin "$branch"
pr=$(gh pr list --head "$branch" --state open --json number -q '.[0].number')
if [ -z "$pr" ]; then
  body=$(git show "$branch:$note" 2>/dev/null || echo "Tâche $task")
  title=$(git log -1 --format=%s "$branch")
  gh pr create -q --base develop --head "$branch" --title "$title" \
    --body "$body"$'\n\n🤖 Generated with [Claude Code](https://claude.com/claude-code)' >/dev/null
  pr=$(gh pr list --head "$branch" --state open --json number -q '.[0].number')
fi
echo "PR #$pr"

if ! grep -q "Verdict: APPROVED" "$review"; then
  gh pr review "$pr" --comment --body-file "$review"
  echo "review asks for changes: not merged"; exit 1
fi

fail() {
  git merge --abort 2>/dev/null || true
  printf '%s\n\nVerdict: CHANGES REQUESTED\n1. %s\n' "Suite rouge sur le résultat fusionné." "$1" > /tmp/integrate-fail.md
  gh pr review "$pr" --comment --body-file /tmp/integrate-fail.md
  echo "RED: $1"; exit 1
}

if ! git merge --no-ff --no-commit "$branch" >/dev/null 2>&1; then
  # The generated index is the usual conflict: regenerate it.
  git checkout --theirs tasks/INDEX.md 2>/dev/null || true
  git add tasks/INDEX.md 2>/dev/null || true
  [ -z "$(git diff --name-only --diff-filter=U)" ] || fail "conflits : $(git diff --name-only --diff-filter=U | tr '\n' ' ')"
fi

cargo test -p laser-studio 2>&1 | grep -E "^test result" > /tmp/integrate-tests.txt || true
[ -s /tmp/integrate-tests.txt ] || fail "cargo test (ne compile pas)"
grep -qv " 0 failed" /tmp/integrate-tests.txt && fail "cargo test"
cargo clippy -q -p laser-studio --all-targets -- -D warnings || fail "clippy"
e2e=$( (cd studio/e2e && npx playwright test --workers=2 2>&1) | tail -4)
echo "$e2e" | grep -q " failed" && fail "e2e : $(echo "$e2e" | grep failed)"

# Green: close the task, append the review, commit the merge.
taskfile=$(ls tasks/"$task"-*.md)
sed -i '' 's/^status: .*/status: done/' "$taskfile"
echo "- $(date +%F): fusionné dans develop (PR #$pr, suite verte)." >> "$taskfile"
[ -f "$note" ] && { printf '\n' >> "$note"; cat "$review" >> "$note"; }
python3 tasks/make_index.py >/dev/null
git add -A studio docs tasks Cargo.lock 2>/dev/null || git add -A studio docs tasks
summary=$(grep -h "test result" /tmp/integrate-tests.txt | head -1)
{ cat "$review"; printf '\n\nSuite sur le résultat fusionné : %s ; e2e : %s\n' "$summary" "$(echo "$e2e" | grep passed | xargs)"; } > /tmp/integrate-review.md
gh pr review "$pr" --comment --body-file /tmp/integrate-review.md
git commit -q -m "Merge $branch: $(git log -1 --format=%s "$branch") (#$pr)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push -q origin develop
echo "merged PR #$pr into develop"
