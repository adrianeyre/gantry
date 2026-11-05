import { Link } from "react-router-dom";
import { usePageTitle } from "../usePageTitle";

export function NotFoundPage() {
  usePageTitle("Page not found");
  return (
    <section aria-labelledby="not-found-heading" className="empty">
      <h1 id="not-found-heading">Page not found</h1>
      <p>There&apos;s nothing at this address. It may have moved, or the link may be mistyped.</p>
      <p>
        <Link to="/" className="button primary">
          Go to the Fleet
        </Link>
      </p>
    </section>
  );
}
