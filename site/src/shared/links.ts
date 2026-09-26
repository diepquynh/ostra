export const REPO = "https://github.com/diepquynh/ostra";
export const REPO_FILE = `${REPO}/blob/master/`;

/** A docs page, from the homepage (both pages are built with relative paths). */
export const docsHref = (page = "", anchor?: string) => `docs/${page ? `#${page}${anchor ? `/${anchor}` : ""}` : ""}`;
