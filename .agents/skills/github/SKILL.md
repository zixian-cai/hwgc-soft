---
name: github
description: "Expert help with GitHub CLI (gh) for managing pull requests, issues, repositories, workflows, and releases. Use this when working with GitHub operations from the command line before resorting to browser use."
---

# GitHub CLI (gh)

## Repository
### Get Repository URL
```
gh repo view --json url -q .url
```

## Pull Requests
### View PRs
```
# Fetch the patch, PR title/body, and list of existing comments (top-level, inline, and reviews)
gh pr diff <PR number> --patch
gh pr view <PR number> --json title,body
gh api --paginate repos/<owner>/<repo>/pulls/<PR number>/comments
gh api --paginate repos/<owner>/<repo>/pulls/<PR number>/reviews
```
