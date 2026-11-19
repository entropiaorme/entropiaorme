// The Terms of Service and the Privacy Policy, read from the repository's
// `legal/` directory at build time. Those Markdown files are the one source
// of the text: the app renders them as they stand, and nothing here restates
// any of it.
import privacyMarkdown from '../../../legal/privacy.md?raw';
import termsMarkdown from '../../../legal/terms.md?raw';

export type LegalDocument = {
	/** The document's heading, shown as the reader's title. */
	title: string;
	/** Everything after the heading, as Markdown. */
	body: string;
};

/** Split a document into its leading `# ` heading and the rest. */
export function parseLegalDocument(markdown: string): LegalDocument {
	const match = /^# (.+)\r?\n([\s\S]*)$/.exec(markdown);
	if (!match) throw new Error('a legal document opens with a level-one heading');
	return { title: match[1].trim(), body: match[2].trim() };
}

export const TERMS_OF_SERVICE: LegalDocument = parseLegalDocument(termsMarkdown);
export const PRIVACY_POLICY: LegalDocument = parseLegalDocument(privacyMarkdown);
