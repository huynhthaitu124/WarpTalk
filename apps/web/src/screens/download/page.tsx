import { Navigate } from "react-router-dom";

/** The desktop download page read GitHub releases on the server; the static build has no release feed, so it goes home. */
export default function DownloadPage() {
  return <Navigate to="/" replace />;
}
