import { describe, expect, it } from 'vitest';
import { PRIVACY_POLICY, parseLegalDocument, TERMS_OF_SERVICE } from './legal';
import { CURRENT_TOS_VERSION } from './tos';

describe('legal documents', () => {
	it('reads each document with its own heading', () => {
		expect(TERMS_OF_SERVICE.title).toBe('Terms of Service');
		expect(PRIVACY_POLICY.title).toBe('Privacy Policy');
		for (const doc of [TERMS_OF_SERVICE, PRIVACY_POLICY]) {
			expect(doc.body).not.toMatch(/^# /m);
		}
	});

	it('states the version the app asks the user to accept', () => {
		// Accepting the documents records CURRENT_TOS_VERSION; the text the
		// user read must carry the same version, or the record says nothing.
		for (const doc of [TERMS_OF_SERVICE, PRIVACY_POLICY]) {
			expect(doc.body.startsWith(`Version ${CURRENT_TOS_VERSION}. Effective `)).toBe(true);
		}
	});

	it('refuses a document without a heading', () => {
		expect(() => parseLegalDocument('Version 1.0.\n\nText.')).toThrow();
	});
});
