---
name: release
description: Release management and deployment
---

This skill is used to create a new release of the project, update the changelog, and deploy the release to the appropriate channels. It automates the process of versioning, tagging, and publishing the release.

## Step 1: Determine the type of release

There are 3 kinds of releases: patch, minor, and major. If not told otherwise, scan the changelog for the latest version and determine the next version based on the changes since the last release. If there are any breaking changes, the next version should be a major release. If there are new features but no breaking changes, it should be a minor release. If there are only bug fixes, it should be a patch release.

## Step 2: Update the versions

Update the version in the Cargo.toml file to the new version. 
Update the version in the changelog to reflect the new version.
Also update the version in the README if it is mentioned there.

## Step 3: Rephrase the changelog entries

Go through the git log since the last release and make sure all changes are reflected in the changelog.

Go through the changelog entries for the new version and rephrase them to be more concise and clear. Remove any unnecessary details and make sure the entries are easy to understand. Highlight the most important changes and improvements in the release.

At this point you should ask the user if he wants to continue with the release or if he wants to make any changes to the changelog or versioning. If the user wants to make changes, go back to step 2 and repeat the process. If the user is satisfied with the changelog and versioning, continue to step 4.

## Step 4: Commit and tag the release

Commit the changes to the Cargo.toml, changelog, and README files with a message like "Release vX.Y.Z". Tag the commit with the new version number.

## Step 5: Push the changes to the remote repository

Push the commit and tag to the remote repository. This will make the new release available to others.

