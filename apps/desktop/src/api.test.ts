import { describe, expect, it } from "vitest";
import { getErrorMessage, redactPaths } from "./api";

describe("renderer path redaction", () => {
  it("redacts Windows and Unix absolute paths", () => {
    expect(redactPaths("C:\\Users\\alice\\secret\\invoice.pdf")).not.toContain(
      "alice",
    );
    expect(redactPaths("/Users/alice/secret/invoice.pdf")).not.toContain(
      "alice",
    );
  });

  it("redacts paths carried by command errors", () => {
    expect(
      getErrorMessage(new Error("cannot read C:\\private\\customer.pdf")),
    ).toContain("[chemin masqué]");
  });

  it("maps stable file-in-use failures without exposing paths", () => {
    expect(
      getErrorMessage("file_in_use: /Users/alice/private/invoice.pdf"),
    ).toBe("Ce fichier est actuellement utilisé par une autre application.");
  });

  it("explains rollback blocked and missing executor without engine jargon", () => {
    expect(getErrorMessage("tcc permission denied")).toBe(
      "macOS n’autorise plus l’accès à ce dossier.",
    );
    expect(getErrorMessage("permission_denied")).toBe(
      "ZEMO a besoin d’accéder à ce dossier pour appliquer l’organisation.",
    );
    expect(getErrorMessage("rollback_blocked")).toBe(
      "Impossible d’annuler ce déplacement car le fichier a été modifié ou remplacé depuis.",
    );
    expect(
      getErrorMessage("L’exécuteur d’application isolé n’est pas disponible dans cette session."),
    ).toBe("L’application des fichiers n’est pas disponible dans cette session.");
  });
});
