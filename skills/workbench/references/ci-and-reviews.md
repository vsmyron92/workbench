# CI triage and reviews on GitLab and GitHub

Use the tools of the project's forge: the GitLab tools when `workbench_projects` shows a `gitlab` path for the project (or for the repository you work in), the GitHub tools when it shows `github`. A project on neither answers that it is not on GitLab or GitHub. If a tool says the service "is not set up" or rejected its token, tell the user; do not look for tokens or edit Workbench's configuration yourself.

## Several repositories in one project

`workbench_projects` adds `repositories` (`id`, `name`, `path`, `default`, `gitlab`, `github`) only for a project that has more than one git repository.

- `repo` is the repository's `id`: `.` for the repository the project root is in, else its folder relative to the root, such as `services/api`. The `path` field is that folder; compare it with the files you changed.
- `repo` exists on every GitLab and GitHub tool and on `workbench_show_diff`, `workbench_set_commit_message` and `workbench_changelists`. Left out (or empty) it means the default repository, the first one listed. An unknown id is refused with a message naming the project and the id.
- Repositories of one project can sit on different GitLab or GitHub projects, even on different forges. Read the `gitlab` and `github` of the repository you work in, then pick the tool family.
- File paths stay relative to the project root, not to the repository: `workbench_show_diff` takes `path: "services/api/src/lib.rs"` together with `repo: "services/api"`. A path that belongs to another repository is refused.
- Not per repository, so no `repo`: files, search, runs, environments, terminals, the Workspace, debugging.
- `projectId` is never needed. It defaults to your project and must be that project.

## GitLab: a failed pipeline

1. `gitlab_pipelines` (`ref`: a branch or tag; `status`: running, pending, success, failed, canceled, skipped or manual; `limit` 1 to 50, default 15). A line reads `#<iid>  id <id>  <status>  <ref>  <sha> ...`. The next tools take the `id`.
2. `gitlab_pipeline_jobs` (`pipelineId`): jobs by stage with status, duration, failure reason and the test summary. Read the first lines too: `YAML errors:` means the pipeline file itself is invalid, so there is no job log to read. `[allowed to fail]` marks a job whose failure does not fail the pipeline. A bridge job prints `downstream pipeline <id>`: call `gitlab_pipeline_jobs` again with that id.
3. `gitlab_test_failures` (`pipelineId`): failed and errored tests from the JUnit report with suite, test, file and failure output (each cut at 4000 characters). "has no test report" means the jobs publish no JUnit file: go to the log.
4. `gitlab_job_log` (`jobId`, `tailLines` 1 to 2000, default 200): the end of the log with colours and timestamps removed. The header says "the log is larger" when its start was not read. The cause is often above the last lines: raise `tailLines` instead of guessing.
5. From a merge request: `gitlab_mr` prints `Pipeline: id <n>`, which is the `pipelineId` for step 2.
6. `gitlab_retry_job` (`jobId`) makes a new job in the same pipeline, so on the same commit. Once a fix is pushed, list again with `gitlab_pipelines`: a new pipeline normally starts by itself.

## GitHub: a failed run

1. `github_runs` (`branch`; `status`: in_progress, queued, completed, success, failure or cancelled; `limit` 1 to 50, default 15). A line reads `run <id>  #<number> ...`: use the `id`.
2. `github_run_jobs` (`runId`): jobs with state, duration and the steps that failed (`failed step: ...`).
3. `github_job_log` (`jobId`, `tailLines` 1 to 2000, default 200): logs exist once the job has finished. For a job still running, or one without a log, you get a message and the job's annotations instead.
4. From a pull request: `github_pr` lists the checks that are not successful, with `job <id>`. Pass that id to `github_job_log` directly.
5. `github_rerun` re-runs the same commit: `runId` for the whole run (with `failedOnly: true` for only its failed jobs) or `jobId` for one job. GitHub has no equivalent of `gitlab_test_failures`: read the log.

## Reviews

- `gitlab_mrs` (`state`: opened (default), merged, closed or all; `search`) and `github_prs` (`state`: open (default), closed, merged or all; `search`) list at most 30, newest activity first. Narrow with `search` instead of expecting more.
- `gitlab_mr` (`iid`) and `github_pr` (`number`): state, branches, merge status, pipeline or checks, approvals or reviews, the description (first 6000 characters) and up to 60 discussion threads. Each thread is marked resolved or unresolved and carries the id you reply to.
- `gitlab_mr_diff` (`iid`, `path`) and `github_pr_diff` (`number`, `path`): a unified diff cut at 200 KB ("diff truncated; pass `path`"); one huge file shows "diff too large". With the branch checked out, `git diff` in your shell is cheaper.
- Comments are mutating and take Markdown. `gitlab_mr_comment` (`iid`, `body`; `discussionId` to reply; `path` with `line` for the new side or `oldLine` for a removed line). `github_pr_comment` (`number`, `body`; `inReplyTo`: the id of the thread's first comment; `path` with `line`, and `side: "LEFT"` for a removed line). Without those extras it is a general comment.
- Creating is mutating and needs the branch pushed already (pushing is yours only when the user asked for it). `gitlab_create_mr` (`title`; `description`, `sourceBranch`, `targetBranch`, `draft`, `removeSourceBranch`, `squash`) and `github_create_pr` (`title`; `body`, `head`, `base`, `draft`) default to the project's current branch and its default branch, and open the result in Workbench: do not also call `workbench_open_url`.

## Messages you may see

- "this project is not on GitLab" or "not on GitHub": the other family is the right one, or the project has no forge.
- "GitLab is not set up", "a GitHub token is needed to ...", "rejected the GitLab token": the user has to connect the service or renew the token in Workbench. Report it and continue without.
- A rate-limit message from the service: wait before the next call; do not loop.
- "this session belongs to project X and cannot act on project Y": you passed another `projectId`. Leave it out.
