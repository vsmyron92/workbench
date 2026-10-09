# Version control and CI

## Git

The **Commit** tool window (Alt+0) lists changes with Changes, Stash and Shelf tabs. It works like CLion:

- stage single lines and make **partial commits**;
- group work into **changelists**;
- **shelve** changes to set them aside;
- interactive **rebase** and **bisect**.

**Alt+9** opens the Git Log. **Ctrl+T** updates the project and **Ctrl+Shift+K** pushes.

In a project with several repositories, the **repository switcher** in the top bar chooses the one these windows and the branch widget work on (see [Projects and files](projects#several-repositories-in-one-project)). Changes of every repository show in the file tree.

## GitLab and GitHub

Both appear as tool windows on the right when a project has that remote (in a project with several repositories, when any of them has; they show the repository selected in the switcher):

- merge requests or pull requests, with diffs, comments and review;
- pipelines, jobs and their logs;
- for GitLab, the failed cases of a JUnit report, each with *Ask agent to fix*;
- issues.

Set the connection up in **Settings → Integrations**. GitLab and GitHub tokens are read from `~/.gitlab_token` and `~/.github_token` unless you point `[secrets.*]` elsewhere. GitHub Enterprise is supported.

## Confluence and Jira

With an Atlassian site configured, pages open in Workbench with inline comments, attachments, mentions, labels and page operations. Jira boards show sprints and move cards through their transitions.

## Workspace cards

The **Workspace** tool window holds the reports, images, PDFs and 3D models that you and your agents produce. They are shown sandboxed. Use *New card…* from the palette to make one.
