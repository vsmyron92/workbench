// Brand marks lucide no longer ships (simplified, single-colour, currentColor).

export function GitLabIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden>
      <path d="m23.6 9.6-.03-.09-3.24-8.46a.84.84 0 0 0-1.6.09l-2.19 6.7H7.46L5.27 1.14a.84.84 0 0 0-1.6-.09L.43 9.51l-.03.09a6.02 6.02 0 0 0 2 6.96l.01.01.03.02 4.93 3.7 2.44 1.84 1.49 1.12a1 1 0 0 0 1.21 0l1.49-1.12 2.44-1.84 4.96-3.72.01-.01a6.03 6.03 0 0 0 2-6.96Z" />
    </svg>
  )
}

/** The GitHub mark (the github slice keeps its own copy in its components). */
export function GitHubIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden>
      <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" />
    </svg>
  )
}

export function ConfluenceIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden>
      <path d="M1.7 17.3c-.25.4-.52.87-.73 1.23a.74.74 0 0 0 .25 1l4.66 2.87a.74.74 0 0 0 1.02-.25c.19-.31.43-.73.7-1.17 1.86-3.07 3.73-2.7 7.1-1.09l4.62 2.2a.74.74 0 0 0 .98-.37l2.22-5.03a.74.74 0 0 0-.36-.96c-.98-.46-2.92-1.37-4.67-2.21C11.2 10.47 5.95 10.67 1.7 17.3Z" />
      <path d="M22.3 6.7c.25-.4.52-.87.73-1.23a.74.74 0 0 0-.25-1L18.12 1.6a.74.74 0 0 0-1.02.25c-.19.31-.43.73-.7 1.17-1.86 3.07-3.73 2.7-7.1 1.09L4.68 1.91a.74.74 0 0 0-.98.37L1.48 7.31a.74.74 0 0 0 .36.96c.98.46 2.92 1.37 4.67 2.21 6.3 3.05 11.54 2.85 15.79-3.78Z" />
    </svg>
  )
}

export function JiraIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden>
      <path d="M11.57 0H.43a5.2 5.2 0 0 0 5.2 5.2h2.05v1.98A5.2 5.2 0 0 0 12.88 12.4V1.3A1.3 1.3 0 0 0 11.57 0Z" />
      <path d="M17.07 5.54H5.93a5.2 5.2 0 0 0 5.2 5.2h2.05v1.98a5.2 5.2 0 0 0 5.2 5.2V6.84a1.3 1.3 0 0 0-1.31-1.3Z" opacity=".8" />
      <path d="M22.57 11.08H11.43a5.2 5.2 0 0 0 5.2 5.2h2.05v1.98a5.2 5.2 0 0 0 5.2 5.2V12.38a1.3 1.3 0 0 0-1.31-1.3Z" opacity=".6" />
    </svg>
  )
}

export function BrandIcon({ name, size = 16 }: { name: 'gitlab' | 'confluence' | 'jira'; size?: number }) {
  if (name === 'gitlab') return <GitLabIcon size={size} />
  if (name === 'confluence') return <ConfluenceIcon size={size} />
  return <JiraIcon size={size} />
}
