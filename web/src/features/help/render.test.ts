// Every shipped page goes through the real Markdown pipeline (server-rendered): a page that
// cannot render, or renders without its headings, fails here instead of in a user's browser.

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { Markdown } from '@/ui/Markdown'
import { PAGES } from './pages'

describe('help pages render', () => {
  for (const page of PAGES) {
    it(page.slug, () => {
      const html = renderToStaticMarkup(createElement(Markdown, { text: page.text }))
      expect(html).toContain(`>${page.title}<`)
      expect(html).toContain('<h2')
    }, 5000)
  }
})
